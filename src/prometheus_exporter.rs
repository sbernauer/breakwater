use std::net::SocketAddr;

use prometheus_exporter::{
    self,
    prometheus::{register_int_gauge, register_int_gauge_vec, IntGauge, IntGaugeVec},
};
use tokio::sync::broadcast;

use crate::statistics::StatisticsInformationEvent;

pub struct PrometheusExporter {
    listen_addr: SocketAddr,
    statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,

    // Prometheus metrics
    metric_ips: IntGauge,
    metric_legacy_ips: IntGauge,
    metric_frame: IntGauge,
    metric_statistic_events: IntGauge,

    metric_connections_for_ip: IntGaugeVec,
    metric_bytes_for_ip: IntGaugeVec,
}

impl PrometheusExporter {
    pub fn new(
        listen_addr: &str,
        statistics_information_rx: broadcast::Receiver<StatisticsInformationEvent>,
    ) -> Self {
        let listen_addr = listen_addr.parse().unwrap_or_else(|_| {
            panic!("Failed to parse prometheus listen address: {listen_addr}",)
        });
        PrometheusExporter {
            listen_addr,
            statistics_information_rx,
            metric_ips: register_int_gauge!("breakwater_ips", "Total number of IPs connected")
                .unwrap(),
            metric_legacy_ips: register_int_gauge!(
                "breakwater_legacy_ips",
                "Total number of legacy (v4) IPs connected"
            )
            .unwrap(),
            metric_frame: register_int_gauge!("breakwater_frame", "Frame number of the VNC server")
                .unwrap(),
            metric_statistic_events: register_int_gauge!(
                "breakwater_statistic_events",
                "Number of statistics events send internally"
            )
            .unwrap(),
            metric_connections_for_ip: register_int_gauge_vec!(
                "breakwater_connections",
                "Number of client connections per IP address",
                &["ip"]
            )
            .unwrap(),
            metric_bytes_for_ip: register_int_gauge_vec!(
                "breakwater_bytes",
                "Number of bytes received",
                &["ip"]
            )
            .unwrap(),
        }
    }

    pub async fn run(&mut self) {
        prometheus_exporter::start(self.listen_addr).expect("Failed to start prometheus exporter");
        while let Ok(event) = self.statistics_information_rx.recv().await {
            self.metric_ips.set(event.ips as i64);
            self.metric_legacy_ips.set(event.legacy_ips as i64);
            self.metric_frame.set(event.frame as i64);
            self.metric_statistic_events
                .set(event.statistic_events as i64);

            // When clients drop a connection the item will be missing in `event.connections_for_ip,
            // but would stay forever in the Prometheus metric
            self.metric_connections_for_ip.reset();
            event
                .connections_for_ip
                .iter()
                .for_each(|(ip, connections)| {
                    self.metric_connections_for_ip
                        .with_label_values(&[&ip.to_string()])
                        .set(*connections as i64)
                });
            self.metric_bytes_for_ip.reset();
            event.bytes_for_ip.iter().for_each(|(ip, bytes)| {
                self.metric_bytes_for_ip
                    .with_label_values(&[&ip.to_string()])
                    .set(*bytes as i64)
            });
        }
    }
}
