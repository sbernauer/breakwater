use std::{
    cmp::max,
    collections::{HashMap, hash_map::Entry},
    fs::File,
    net::IpAddr,
    time::Duration,
};

use color_eyre::eyre::{self, Context};
use serde::{Deserialize, Serialize};
use simple_moving_average::{SMA, SingleSumSMA};
use tokio::{
    sync::{broadcast, mpsc},
    time::interval,
};

pub const STATS_REPORT_INTERVAL: Duration = Duration::from_millis(1000);
pub const STATS_SLIDING_WINDOW_SIZE: usize = 5;
pub const STATISTICS_SEND_ERR: &str = "failed to send on statistics channel";
pub const STATISTICS_INFO_SEND_ERR: &str = "failed to send on statistics information channel";
#[cfg(feature = "vnc")]
pub const STATISTICS_INFO_RECV_ERR: &str = "failed to receive on statistics information channel";

#[derive(Debug)]
pub enum StatisticsEvent {
    ConnectionCreated {
        ip: IpAddr,
    },
    ConnectionClosed {
        ip: IpAddr,
    },
    ConnectionDenied {
        ip: IpAddr,
    },
    BytesRead {
        ip: IpAddr,
        bytes: u64,
    },
    #[cfg(feature = "vnc")]
    VncFrameRendered,
}

pub enum StatisticsSaveMode {
    Disabled,
    Enabled { save_file: String, interval_s: u64 },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StatisticsInformationEvent {
    pub frame: u64,
    pub connections: u32,
    pub ips_v6: u32,
    pub ips_v4: u32,
    pub bytes: u64,
    pub fps: u64,
    pub bytes_per_s: u64,

    pub connections_for_ip: HashMap<IpAddr, u32>,
    pub denied_connections_for_ip: HashMap<IpAddr, u32>,
    pub bytes_for_ip: HashMap<IpAddr, u64>,

    pub statistic_events: u64,
}

pub struct Statistics {
    statistics_rx: mpsc::Receiver<StatisticsEvent>,
    statistics_information_tx: broadcast::Sender<StatisticsInformationEvent>,
    statistic_events: u64,

    frame: u64,
    connections_for_ip: HashMap<IpAddr, u32>,
    denied_connections_for_ip: HashMap<IpAddr, u32>,
    bytes_for_ip: HashMap<IpAddr, u64>,

    bytes_per_s_window: SingleSumSMA<u64, u64, STATS_SLIDING_WINDOW_SIZE>,
    fps_window: SingleSumSMA<u64, u64, STATS_SLIDING_WINDOW_SIZE>,

    statistics_save_mode: StatisticsSaveMode,
}

impl StatisticsInformationEvent {
    fn save_to_file(&self, file_name: &str) -> eyre::Result<()> {
        // TODO Check if we can use tokio's File here. This needs some integration with serde_json though
        // This operation is also called very infrequently
        let file = File::create(file_name)
            .with_context(|| format!("failed to create statistics save file at {file_name}"))?;
        serde_json::to_writer(file, &self)
            .context("failed to write statistics to file at {file_name}")?;

        Ok(())
    }

    fn load_from_file(file_name: &str) -> eyre::Result<Self> {
        let file = File::open(file_name)
            .with_context(|| format!("failed to load statistic from file '{file_name}'"))?;
        serde_json::from_reader(file)
            .with_context(|| format!("failed to deserialize statistics from file '{file_name}'"))
    }
}

impl Statistics {
    pub fn new(
        statistics_rx: mpsc::Receiver<StatisticsEvent>,
        statistics_information_tx: broadcast::Sender<StatisticsInformationEvent>,
        statistics_save_mode: StatisticsSaveMode,
    ) -> Self {
        let mut statistics = Statistics {
            statistics_rx,
            statistics_information_tx,
            statistic_events: 0,
            frame: 0,
            connections_for_ip: HashMap::new(),
            denied_connections_for_ip: HashMap::new(),
            bytes_for_ip: HashMap::new(),
            bytes_per_s_window: SingleSumSMA::new(),
            fps_window: SingleSumSMA::new(),
            statistics_save_mode,
        };

        if let StatisticsSaveMode::Enabled { save_file, .. } = &statistics.statistics_save_mode {
            // There might not be a save point on first start
            if let Ok(save_point) = StatisticsInformationEvent::load_from_file(save_file) {
                statistics.statistic_events = save_point.statistic_events;
                statistics.frame = save_point.frame;
                statistics.bytes_for_ip = save_point.bytes_for_ip;
            }
        }

        statistics
    }

    pub async fn run(&mut self) -> eyre::Result<()> {
        let mut statistics_information_event = StatisticsInformationEvent::default();

        let mut stats_report = interval(STATS_REPORT_INTERVAL);
        let (mut stats_save, save_file) = match &self.statistics_save_mode {
            StatisticsSaveMode::Disabled => (interval(Duration::MAX), None),
            StatisticsSaveMode::Enabled {
                save_file,
                interval_s,
            } => (
                interval(Duration::from_secs(*interval_s)),
                Some(save_file.clone()),
            ),
        };

        loop {
            tokio::select! {
                // Cancellation safety: mpsc::Receiver::recv is cancellation safe
                maybe_event = self.statistics_rx.recv() => {
                    let Some(event) = maybe_event else {
                        // `self.statistics_rx` is closed, program is terminating
                        return Ok(());
                    };
                    self.process_statistics_event(event);
                },
                // Cancellation safety: This method is cancellation safe. If tick is used as the branch in a tokio::select!
                // and another branch completes first, then no tick has been consumed.
                _ = stats_report.tick() => {
                    statistics_information_event = self.calculate_statistics_information_event(
                        &statistics_information_event,
                        STATS_REPORT_INTERVAL,
                    );
                    self.statistics_information_tx
                        .send(statistics_information_event.clone())
                        .map_err(Box::new)
                        .context(STATISTICS_INFO_SEND_ERR)?;
                },
                // Cancellation safety: This method is cancellation safe. If tick is used as the branch in a tokio::select!
                // and another branch completes first, then no tick has been consumed.
                _ = stats_save.tick() => {
                    if let Some(save_file) = &save_file {
                        statistics_information_event.save_to_file(save_file)?;
                    }
                },
            };
        }
    }

    fn process_statistics_event(&mut self, event: StatisticsEvent) {
        self.statistic_events += 1;
        match event {
            StatisticsEvent::ConnectionCreated { ip } => {
                *self.connections_for_ip.entry(ip).or_insert(0) += 1;
            }
            StatisticsEvent::ConnectionClosed { ip } => {
                if let Entry::Occupied(mut o) = self.connections_for_ip.entry(ip) {
                    let connections = o.get_mut();
                    *connections -= 1;
                    if *connections == 0 {
                        o.remove_entry();
                    }
                }
            }
            StatisticsEvent::ConnectionDenied { ip } => {
                *self.denied_connections_for_ip.entry(ip).or_insert(0) += 1;
            }
            StatisticsEvent::BytesRead { ip, bytes } => {
                *self.bytes_for_ip.entry(ip).or_insert(0) += bytes;
            }
            #[cfg(feature = "vnc")]
            StatisticsEvent::VncFrameRendered => self.frame += 1,
        }
    }

    fn calculate_statistics_information_event(
        &mut self,
        prev: &StatisticsInformationEvent,
        elapsed: Duration,
    ) -> StatisticsInformationEvent {
        let elapsed_ms = max(1, elapsed.as_millis()) as u64;
        let frame = self.frame;
        let connections = self.connections_for_ip.values().sum();
        let [ips_v6, ips_v4] = self
            .connections_for_ip
            .keys()
            .fold([0, 0], |[v6, v4], e| match e {
                IpAddr::V6(_) => [v6 + 1, v4],
                IpAddr::V4(_) => [v6, v4 + 1],
            });
        let bytes = self.bytes_for_ip.values().sum();
        self.bytes_per_s_window
            .add_sample((bytes - prev.bytes) * 1000 / elapsed_ms);
        self.fps_window
            .add_sample((frame - prev.frame) * 1000 / elapsed_ms);
        let statistic_events = self.statistic_events;

        StatisticsInformationEvent {
            frame,
            connections,
            ips_v6,
            ips_v4,
            bytes,
            fps: self.fps_window.get_average(),
            bytes_per_s: self.bytes_per_s_window.get_average(),
            connections_for_ip: self.connections_for_ip.clone(),
            denied_connections_for_ip: self.denied_connections_for_ip.clone(),
            bytes_for_ip: self.bytes_for_ip.clone(),
            statistic_events,
        }
    }
}
