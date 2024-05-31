#![allow(internal_features)]
#![feature(core_intrinsics)]
// #![feature(new_uninit)]

use std::{
    collections::VecDeque, intrinsics, mem::ManuallyDrop, net::TcpListener, os::fd::AsRawFd,
    thread, time::Duration,
};

use io_uring::{opcode, squeue, types::Fd, IoUring};
use snafu::{ResultExt, Snafu};
use tracing::Level;
use tracing_subscriber::EnvFilter;

const LISTENER_ADDRESS: &str = "[::]:1234";

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("failed to set global tracing subscriber"))]
    SetGlobalTracingSubscriber {
        source: tracing::subscriber::SetGlobalDefaultError,
    },

    #[snafu(display("failed to bind to address {address}"))]
    BindAddress {
        source: std::io::Error,
        address: String,
    },

    #[snafu(display("failed to build uring"))]
    BuildUring { source: std::io::Error },

    #[snafu(display("failed to submit to ring"))]
    RingSubmit { source: std::io::Error },

    #[snafu(display("failed to accept client"))]
    AcceptClient { source: std::io::Error },

    #[snafu(display("failed to submit and wait"))]
    SubmitAndWait { source: std::io::Error },
}

fn setup_logging() -> Result<(), Error> {
    if cfg!(debug_assertions) {
        let filter = EnvFilter::builder()
            .with_default_directive(Level::DEBUG.into())
            .from_env_lossy();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .compact()
            //.pretty()
            //.with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .without_time()
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .context(SetGlobalTracingSubscriberSnafu)?;
    } else {
        let filter = EnvFilter::builder()
            .with_default_directive(Level::INFO.into())
            .from_env_lossy();

        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .compact()
            .with_thread_names(true)
            .with_target(false)
            .with_thread_names(true)
            .finish();
        tracing::subscriber::set_global_default(subscriber)
            .context(SetGlobalTracingSubscriberSnafu)?;
    }

    Ok(())
}

fn main() -> Result<(), Error> {
    setup_logging()?;

    if cfg!(debug_assertions) {
        tracing::trace!("enabled");
        tracing::debug!("enabled");
        tracing::info!("enabled");
        tracing::warn!("enabled");
        tracing::error!("enabled");
    }

    let workers = num_cpus::get();

    let (tx, rx) = std::sync::mpsc::channel();
    let handles = (0..workers)
        .map(|_| {
            let tx = tx.clone();
            thread::spawn(move || main_ring(Some(tx), vec![]))
        })
        .collect::<Vec<_>>();
    drop(tx);

    let worker_fds = rx.into_iter().collect::<Vec<_>>();

    tracing::debug!(?worker_fds);

    main_ring(None, worker_fds)?;

    for handle in handles {
        handle.join().unwrap()?;
    }

    Ok(())
}

fn main_ring(
    fd_report: Option<std::sync::mpsc::Sender<i32>>,
    worker_fds: Vec<i32>,
) -> Result<(), Error> {
    let mut ring = new_uring(1024, 1024)?;
    let mut backlog = VecDeque::default();

    match fd_report {
        Some(fd_report) => {
            fd_report
                .send(ring.as_raw_fd())
                .expect("main thread shut down");
            drop(fd_report);
        }
        None => {
            let listener = TcpListener::bind(LISTENER_ADDRESS).context(BindAddressSnafu {
                address: LISTENER_ADDRESS,
            })?;
            let accept_ms = opcode::AcceptMulti::new(Fd(listener.as_raw_fd()))
                .build()
                .user_data(UserData::Accept { listener }.into());

            backlog.push_back(accept_ms);
        }
    }

    let mut worker_fds_cycle = worker_fds.into_iter().cycle();
    let res: Result<(), Error> = 'ring_loop: loop {
        ring.completion().sync();
        if backlog.is_empty() || ring.completion().is_full() {
            handle_cqes(&mut ring, &mut worker_fds_cycle, &mut backlog)?;
        }

        while let Some(entry) = backlog.pop_front() {
            if let Err(_) = unsafe { ring.submission_shared().push(&entry) } {
                backlog.push_front(entry);

                unsafe { ring.submission_shared().sync() };
                ring.submit().context(RingSubmitSnafu)?;
                continue 'ring_loop;
            }
        }

        ring.submission().sync();
        while let Err(err) = ring.submit_and_wait(1) {
            if err.raw_os_error().unwrap_or_default() == libc::EBUSY {
                // cq is full, we have to reap cqes before submitting again
                continue 'ring_loop;
            }

            if err.raw_os_error().unwrap_or_default() == libc::EAGAIN {
                tracing::warn!("unable to submit to ring. System overload? retrying..");
                thread::sleep(Duration::from_millis(100));
                continue;
            }

            break 'ring_loop Err(Error::SubmitAndWait { source: err });
        }
    };

    tracing::info!("Exiting...");
    res
}

fn handle_cqes(
    ring: &mut IoUring,
    worker_fds_cycle: &mut impl Iterator<Item = i32>,
    backlog: &mut VecDeque<squeue::Entry>,
) -> Result<(), Error> {
    let (_submitter, mut sq, mut cq) = ring.split();

    for cqe in &mut cq {
        let user_data = UserData::from_user_data(cqe.user_data());
        let Some(mut user_data) = user_data else {
            continue;
        };

        match user_data.as_mut() {
            UserData::SendClient { fd } => {
                let fd = *fd;
                tracing::info!("got client from master: {fd}");

                let mut buf = vec![0u8; 265 * 1024].into_boxed_slice();
                let read = opcode::Recv::new(Fd(fd), buf.as_mut_ptr(), buf.len() as u32)
                    .build()
                    .user_data(UserData::Read { buf, fd }.into());

                if let Err(_) = unsafe { sq.push(&read) } {
                    backlog.push_back(read);
                }
            }
            UserData::Accept { listener } => {
                match cqe.result() {
                    e if e < 0 => {
                        let err = std::io::Error::from_raw_os_error(-e);
                        tracing::error!("unable to accept client: {err}");
                        unsafe { drop(ManuallyDrop::take(&mut user_data)) };
                        return Err(Error::AcceptClient { source: err });
                    }
                    0 => unreachable!(),
                    fd => {
                        tracing::info!("new client: {fd}");

                        let ring_fd = worker_fds_cycle.next().unwrap();
                        let msg = opcode::MsgRingData::new(
                            Fd(ring_fd),
                            0,
                            UserData::SendClient { fd }.into(),
                            None,
                        )
                        .build()
                        .user_data(0);

                        if let Err(_) = unsafe { sq.push(&msg) } {
                            backlog.push_back(msg);
                        }
                    }
                }

                if intrinsics::unlikely(io_uring::cqueue::more(cqe.flags())) {
                    // kernel wont emit any more cqe for this request
                    // so we rerequest
                    let recv = opcode::AcceptMulti::new(Fd(listener.as_raw_fd()))
                        .build()
                        .user_data(cqe.user_data())
                        .into();

                    if let Err(_) = unsafe { sq.push(&recv) } {
                        backlog.push_back(recv);
                    }
                }
            }
            UserData::Read { buf, fd } => match cqe.result() {
                e if e < 0 => {
                    let err = std::io::Error::from_raw_os_error(-e);
                    tracing::error!("unable to read from socket: {err}");

                    let fd = *fd;
                    let _user_data = unsafe { ManuallyDrop::<Box<UserData>>::take(&mut user_data) };
                    let close = opcode::Close::new(Fd(fd)).build().user_data(0);

                    if let Err(_) = unsafe { sq.push(&close) } {
                        backlog.push_back(close);
                    }
                    continue;
                }
                0 => {
                    let fd = *fd;
                    tracing::info!("socket closed: {fd}");

                    let _user_data = unsafe { ManuallyDrop::<Box<UserData>>::take(&mut user_data) };
                    let close = opcode::Close::new(Fd(fd)).build().user_data(0);

                    if let Err(_) = unsafe { sq.push(&close) } {
                        backlog.push_back(close);
                    }
                    continue;
                }
                bytes => {
                    tracing::debug!("received {bytes} bytes from {fd}");

                    let read = opcode::Recv::new(Fd(*fd), buf.as_mut_ptr(), buf.len() as u32)
                        .build()
                        .user_data(cqe.user_data());

                    if let Err(_) = unsafe { sq.push(&read) } {
                        backlog.push_back(read);
                    }
                }
            },
        }
    }
    Ok(())
}

pub enum UserData {
    Accept { listener: TcpListener },
    SendClient { fd: i32 },
    Read { buf: Box<[u8]>, fd: i32 },
}

impl UserData {
    pub fn from_user_data(user_data: u64) -> Option<ManuallyDrop<Box<UserData>>> {
        let ptr = user_data as *mut UserData;
        if ptr.is_null() {
            return None;
        }

        let boxed = unsafe { Box::from_raw(ptr) };
        Some(ManuallyDrop::new(boxed))
    }
}

impl Into<u64> for UserData {
    fn into(self) -> u64 {
        Box::into_raw(Box::new(self)) as u64
    }
}

impl Into<u64> for Box<UserData> {
    fn into(self) -> u64 {
        Box::into_raw(self) as u64
    }
}

fn new_uring(sq_size: u32, cq_size: u32) -> Result<io_uring::IoUring, Error> {
    io_uring::IoUring::builder()
        .setup_single_issuer()
        .setup_coop_taskrun()
        .setup_defer_taskrun()
        .setup_cqsize(cq_size)
        .build(sq_size)
        .context(BuildUringSnafu)
}
