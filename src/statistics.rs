use serde::{Deserialize, Serialize};
use simple_moving_average::{SingleSumSMA, SMA};
use std::{
    cmp::max,
    collections::{hash_map::Entry, HashMap},
    fs::File,
    net::IpAddr,
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc::Receiver};

pub const STATS_REPORT_INTERVAL: Duration = Duration::from_millis(1000);
pub const STATS_SLIDING_WINDOW_SIZE: usize = 5;

#[derive(Debug)]
pub enum StatisticsEvent {
    ConnectionCreated { ip: IpAddr },
    ConnectionClosed { ip: IpAddr },
    BytesRead { ip: IpAddr, bytes: u64 },
    FrameRendered,
}

pub enum StatisticsSaveMode {
    Disabled,
    Enabled { save_file: String, interval_s: u64 },
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct StatisticsInformationEvent {
    pub frame: u64,
    pub connections: u32,
    pub ips: u32,
    pub legacy_ips: u32,
    pub bytes: u64,
    pub fps: u64,
    pub bytes_per_s: u64,

    pub connections_for_ip: HashMap<IpAddr, u32>,
    pub bytes_for_ip: HashMap<IpAddr, u64>,

    pub statistic_events: u64,
}

pub struct Statistics {
    statistics_rx: Receiver<StatisticsEvent>,
    statistics_information_tx: broadcast::Sender<StatisticsInformationEvent>,
    statistic_events: u64,

    frame: u64,
    connections_for_ip: HashMap<IpAddr, u32>,
    bytes_for_ip: HashMap<IpAddr, u64>,

    bytes_per_s_window: SingleSumSMA<u64, u64, STATS_SLIDING_WINDOW_SIZE>,
    fps_window: SingleSumSMA<u64, u64, STATS_SLIDING_WINDOW_SIZE>,

    statistics_save_mode: StatisticsSaveMode,
}

impl StatisticsInformationEvent {
    fn save_to_file(&self, file_name: &str) -> std::io::Result<()> {
        // TODO Check if we can use tokio's File here. This needs some integration with serde_json though
        // This operation is also called very infrequently
        let file = File::create(file_name)?;
        serde_json::to_writer(file, &self)?;

        Ok(())
    }

    fn load_from_file(file_name: &str) -> std::io::Result<Self> {
        let file = File::open(file_name)?;
        Ok(serde_json::from_reader(file)?)
    }
}

impl Statistics {
    pub fn new(
        statistics_rx: Receiver<StatisticsEvent>,
        statistics_information_tx: broadcast::Sender<StatisticsInformationEvent>,
        statistics_save_mode: StatisticsSaveMode,
    ) -> std::io::Result<Self> {
        let mut statistics = Statistics {
            statistics_rx,
            statistics_information_tx,
            statistic_events: 0,
            frame: 0,
            connections_for_ip: HashMap::new(),
            bytes_for_ip: HashMap::new(),
            bytes_per_s_window: SingleSumSMA::new(),
            fps_window: SingleSumSMA::new(),
            statistics_save_mode,
        };

        if let StatisticsSaveMode::Enabled { save_file, .. } = &statistics.statistics_save_mode {
            if let Ok(save_point) = StatisticsInformationEvent::load_from_file(save_file) {
                statistics.statistic_events = save_point.statistic_events;
                statistics.frame = save_point.frame;
                statistics.bytes_for_ip = save_point.bytes_for_ip;
            }
        }

        Ok(statistics)
    }

    pub async fn start(&mut self) -> std::io::Result<()> {
        let mut last_stat_report = Instant::now();
        let mut last_save_file_written = Instant::now();
        let mut statistics_information_event = StatisticsInformationEvent::default();

        while let Some(statistics_update) = self.statistics_rx.recv().await {
            self.statistic_events += 1;
            match statistics_update {
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
                StatisticsEvent::BytesRead { ip, bytes } => {
                    *self.bytes_for_ip.entry(ip).or_insert(0) += bytes;
                }
                StatisticsEvent::FrameRendered => self.frame += 1,
            }

            // As there is an event for every frame we are guaranteed to land here every second
            let last_stat_report_elapsed = last_stat_report.elapsed();
            if last_stat_report_elapsed > STATS_REPORT_INTERVAL {
                last_stat_report = Instant::now();
                statistics_information_event = self.calculate_statistics_information_event(
                    &statistics_information_event,
                    last_stat_report_elapsed,
                );
                self.statistics_information_tx
                    .send(statistics_information_event.clone())
                    .expect("Statistics information channel full (or disconnected)");

                if let StatisticsSaveMode::Enabled {
                    save_file,
                    interval_s,
                } = &self.statistics_save_mode
                {
                    if last_save_file_written.elapsed() > Duration::from_secs(*interval_s) {
                        last_save_file_written = Instant::now();
                        statistics_information_event.save_to_file(save_file)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn calculate_statistics_information_event(
        &mut self,
        prev: &StatisticsInformationEvent,
        elapsed: Duration,
    ) -> StatisticsInformationEvent {
        let elapsed_ms = max(1, elapsed.as_millis()) as u64;
        let frame = self.frame;
        let connections = self.connections_for_ip.values().sum();
        let ips = self.connections_for_ip.len() as u32;
        let legacy_ips = self
            .connections_for_ip
            .keys()
            .filter(|ip| ip.is_ipv4())
            .count() as u32;
        let bytes = self.bytes_for_ip.values().sum();
        self.bytes_per_s_window
            .add_sample((bytes - prev.bytes) * 1000 / elapsed_ms);
        self.fps_window
            .add_sample((frame - prev.frame) * 1000 / elapsed_ms);
        let statistic_events = self.statistic_events;

        StatisticsInformationEvent {
            frame,
            connections,
            ips,
            legacy_ips,
            bytes,
            fps: self.fps_window.get_average(),
            bytes_per_s: self.bytes_per_s_window.get_average(),
            connections_for_ip: self.connections_for_ip.clone(),
            bytes_for_ip: self.bytes_for_ip.clone(),
            statistic_events,
        }
    }
}
