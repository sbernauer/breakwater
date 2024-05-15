use std::net::AddrParseError;

use prometheus_exporter::{
    self,
    prometheus::{register_int_gauge, register_int_gauge_vec, IntGauge, IntGaugeVec},
};
use snafu::{ResultExt, Snafu};
use tokio::sync::broadcast;

use crate::statistics::StatisticsInformationEvent;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Failed to parse Prometheus listen address {listen_address:?}"))]
    ParseListenAddress {
        source: AddrParseError,
        listen_address: String,
    },

    #[snafu(display("Failed to start Prometheus server"))]
    StartPrometheusServer { source: prometheus_exporter::Error },

    #[snafu(display("Failed to register prometheus gauge {name:?}"))]
    RegisterPrometheusGauge {
        source: prometheus_exporter::prometheus::Error,
        name: String,
    },
}

pub struct PrometheusExporter {
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
    ) -> Result<Self, Error> {
        let listen_addr = listen_addr.parse().context(ParseListenAddressSnafu {
            listen_address: listen_addr.to_string(),
        })?;

        prometheus_exporter::start(listen_addr).context(StartPrometheusServerSnafu)?;

        Ok(PrometheusExporter {
            statistics_information_rx,
            metric_legacy_ips: register_int_gauge(
                "breakwater_ips",
                "Total number of IPs connected",
            )?,
            metric_ips: register_int_gauge(
                "breakwater_legacy_ips",
                "Total number of legacy (v4) IPs connected",
            )?,
            metric_frame: register_int_gauge("breakwater_frame", "Frame number of the VNC server")?,
            metric_statistic_events: register_int_gauge(
                "breakwater_statistic_events",
                "Number of statistics events send internally",
            )?,
            metric_connections_for_ip: register_int_gauge_vec(
                "breakwater_connections",
                "Number of client connections per IP address",
                &["ip"],
            )?,
            metric_bytes_for_ip: register_int_gauge_vec(
                "breakwater_bytes",
                "Number of bytes received per IP address",
                &["ip"],
            )?,
        })
    }

    pub async fn run(&mut self) {
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

fn register_int_gauge(name: &str, description: &str) -> Result<IntGauge, Error> {
    register_int_gauge!(name, description).context(RegisterPrometheusGaugeSnafu {
        name: name.to_string(),
    })
}

fn register_int_gauge_vec(
    name: &str,
    description: &str,
    label_names: &[&str],
) -> Result<IntGaugeVec, Error> {
    register_int_gauge_vec!(name, description, label_names).context(RegisterPrometheusGaugeSnafu {
        name: name.to_string(),
    })
}
