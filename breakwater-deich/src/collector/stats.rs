//! Collector-side statistics: folds each worker's periodic snapshot into persistent, per-IP grand
//! totals that survive both worker and collector restarts.
//!
//! # Contract with the worker's aggregator
//!
//! Each worker runs breakwater's [`Statistics`] aggregator and streams its
//! [`StatisticsInformationEvent`] snapshots. We rely on two properties of that aggregator, which
//! hold as long as the worker keeps the connection open (one "session"):
//!
//! - `bytes_for_ip` and `denied_connections_for_ip` are **cumulative and monotonic within a
//!   session** — they only ever grow. We therefore fold the *delta* since the worker's previous
//!   snapshot into the grand totals, so a cumulative counter isn't counted many times over.
//! - `connections_for_ip` is a **live gauge** (it goes up and down as connections open/close), so we
//!   sum the latest snapshots across workers rather than accumulating deltas.
//!
//! On disconnect a worker's baseline is dropped ([`Self::forget`]); its counters reset to zero when
//! it reconnects, and the first post-reconnect snapshot then counts in full from that zero.
//!
//! [`Statistics`]: breakwater::statistics::Statistics

use std::{collections::HashMap, net::IpAddr};

use breakwater::statistics::{STATS_REPORT_INTERVAL, StatisticsInformationEvent};
use uuid::Uuid;

/// Holds both the live, per-worker view and the persistent grand totals.
#[derive(Default)]
pub struct CollectorStatistics {
    /// The latest snapshot from each connected worker, keyed by UUID. Used for the live connection
    /// gauge and as the per-worker baseline for delta accumulation. Removed on disconnect.
    latest_per_worker: HashMap<Uuid, StatisticsInformationEvent>,

    /// Monotonic grand totals, accumulated from per-worker deltas. Persisted to the save file and
    /// seeded from it on startup, so the "big numbers" outlive any restart.
    total_bytes_for_ip: HashMap<IpAddr, u64>,
    total_denied_for_ip: HashMap<IpAddr, u32>,
}

impl CollectorStatistics {
    /// Seeds the grand totals from a previously saved snapshot (the live per-worker view always
    /// starts empty — it's rebuilt as workers (re)connect).
    pub fn from_save_point(save_point: StatisticsInformationEvent) -> Self {
        Self {
            total_bytes_for_ip: save_point.bytes_for_ip,
            total_denied_for_ip: save_point.denied_connections_for_ip,
            ..Default::default()
        }
    }

    /// Records a worker's fresh snapshot: folds the per-IP increase since its previous snapshot
    /// (zero if this is its first) into the grand totals, then stores it as the new baseline.
    pub fn record(&mut self, worker_id: Uuid, event: StatisticsInformationEvent) {
        let previous = self.latest_per_worker.get(&worker_id);

        for (&ip, &bytes) in &event.bytes_for_ip {
            let baseline = previous
                .and_then(|p| p.bytes_for_ip.get(&ip))
                .copied()
                .unwrap_or(0);
            // Monotonic within a session, so `bytes >= baseline`; `saturating_sub` only guards the
            // (shouldn't-happen) case of a counter going backwards without a disconnect in between.
            *self.total_bytes_for_ip.entry(ip).or_default() += bytes.saturating_sub(baseline);
        }
        for (&ip, &denied) in &event.denied_connections_for_ip {
            let baseline = previous
                .and_then(|p| p.denied_connections_for_ip.get(&ip))
                .copied()
                .unwrap_or(0);
            let total = self.total_denied_for_ip.entry(ip).or_default();
            *total = total.saturating_add(denied.saturating_sub(baseline));
        }

        self.latest_per_worker.insert(worker_id, event);
    }

    /// Drops a disconnected worker's baseline so its next session accumulates from zero again. Its
    /// already-folded bytes stay in the grand totals.
    pub fn forget(&mut self, worker_id: Uuid) {
        self.latest_per_worker.remove(&worker_id);
    }

    /// Builds the event published to the sinks: persistent grand totals for bytes/denied, plus the
    /// live connection gauge summed across currently-connected workers. `previous_bytes` carries
    /// the last tick's total so we can derive a per-second rate at the collector.
    pub fn published_event(&self, previous_bytes: &mut u64) -> StatisticsInformationEvent {
        let mut connections_for_ip: HashMap<IpAddr, u32> = HashMap::new();
        let mut statistic_events = 0;
        for snapshot in self.latest_per_worker.values() {
            for (&ip, &connections) in &snapshot.connections_for_ip {
                *connections_for_ip.entry(ip).or_default() += connections;
            }
            statistic_events += snapshot.statistic_events;
        }

        let connections = connections_for_ip.values().sum();
        let [ips_v6, ips_v4] = connections_for_ip
            .keys()
            .fold([0u32, 0u32], |[v6, v4], ip| match ip {
                IpAddr::V6(_) => [v6 + 1, v4],
                IpAddr::V4(_) => [v6, v4 + 1],
            });

        let bytes: u64 = self.total_bytes_for_ip.values().sum();
        // Rate over one report interval, saturating since a worker dropping out can't shrink the
        // (monotonic) total, but a freshly seeded total on startup can jump the first `previous`.
        let elapsed_secs = STATS_REPORT_INTERVAL.as_secs().max(1);
        let bytes_per_s = bytes.saturating_sub(*previous_bytes) / elapsed_secs;
        *previous_bytes = bytes;

        StatisticsInformationEvent {
            connections,
            ips_v6,
            ips_v4,
            bytes,
            bytes_per_s,
            connections_for_ip,
            denied_connections_for_ip: self.total_denied_for_ip.clone(),
            bytes_for_ip: self.total_bytes_for_ip.clone(),
            statistic_events,
            // Workers don't render, so there's no frame/fps to report at the collector.
            frame: 0,
            fps: 0,
        }
    }

    /// Builds the event written to the save file: only the persistent grand totals matter (the live
    /// view is rebuilt from reconnecting workers), so the other fields stay at their defaults.
    pub fn save_point(&self) -> StatisticsInformationEvent {
        StatisticsInformationEvent {
            bytes: self.total_bytes_for_ip.values().sum(),
            bytes_for_ip: self.total_bytes_for_ip.clone(),
            denied_connections_for_ip: self.total_denied_for_ip.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a worker snapshot carrying per-IP byte and (live) connection counts.
    fn snapshot(
        bytes_for_ip: &[(IpAddr, u64)],
        connections_for_ip: &[(IpAddr, u32)],
    ) -> StatisticsInformationEvent {
        StatisticsInformationEvent {
            bytes_for_ip: bytes_for_ip.iter().copied().collect(),
            connections_for_ip: connections_for_ip.iter().copied().collect(),
            statistic_events: 1,
            ..Default::default()
        }
    }

    #[test]
    fn accumulates_per_ip_totals_and_live_connections_across_workers() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let v6: IpAddr = "::1".parse().unwrap();
        let (w1, w2) = (Uuid::from_u128(1), Uuid::from_u128(2));

        // `v4` hit both workers; `v6` only the second. Totals and the shared IP must add up.
        let mut stats = CollectorStatistics::default();
        stats.record(w1, snapshot(&[(v4, 100)], &[(v4, 2)]));
        stats.record(w2, snapshot(&[(v4, 50), (v6, 7)], &[(v4, 1), (v6, 3)]));

        let mut previous_bytes = 0;
        let event = stats.published_event(&mut previous_bytes);

        // Bytes are the persistent grand total.
        assert_eq!(event.bytes_for_ip[&v4], 150);
        assert_eq!(event.bytes_for_ip[&v6], 7);
        assert_eq!(event.bytes, 157);
        // Connections are the live gauge, summed across currently-connected workers.
        assert_eq!(event.connections_for_ip[&v4], 3);
        assert_eq!(event.connections, 6);
        assert_eq!(event.ips_v4, 1);
        assert_eq!(event.ips_v6, 1);
        assert_eq!(event.statistic_events, 2);
        // First tick: the whole total counts as this interval's throughput.
        assert_eq!(event.bytes_per_s, 157 / STATS_REPORT_INTERVAL.as_secs().max(1));
        assert_eq!(previous_bytes, 157);
    }

    #[test]
    fn folds_deltas_so_a_cumulative_counter_is_not_double_counted() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();

        // A worker's counter is cumulative within a session: only the increase between consecutive
        // snapshots is folded into the grand total.
        stats.record(w, snapshot(&[(v4, 100)], &[]));
        stats.record(w, snapshot(&[(v4, 175)], &[]));
        assert_eq!(stats.total_bytes_for_ip[&v4], 175);
    }

    #[test]
    fn worker_restart_keeps_totals_without_dipping_or_double_counting() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();

        stats.record(w, snapshot(&[(v4, 100)], &[(v4, 1)]));
        // Worker disconnects: its baseline is dropped, but its folded bytes stay in the total.
        stats.forget(w);
        assert_eq!(stats.total_bytes_for_ip[&v4], 100);

        // It reconnects (same UUID) with a counter reset to zero. The first post-restart snapshot
        // counts in full, so the total grows by exactly the new traffic — no dip, no double count.
        stats.record(w, snapshot(&[(v4, 30)], &[(v4, 1)]));
        assert_eq!(stats.total_bytes_for_ip[&v4], 130);
    }

    #[test]
    fn accumulates_denied_connections() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let denied = |n: u32| StatisticsInformationEvent {
            denied_connections_for_ip: [(v4, n)].into_iter().collect(),
            ..Default::default()
        };

        let mut stats = CollectorStatistics::default();
        stats.record(w, denied(3));
        stats.record(w, denied(5)); // cumulative -> grand total +2
        assert_eq!(stats.total_denied_for_ip[&v4], 5);
        assert_eq!(stats.published_event(&mut 0).denied_connections_for_ip[&v4], 5);
    }

    #[test]
    fn save_point_seeds_grand_totals_on_restart() {
        let v4: IpAddr = "1.2.3.4".parse().unwrap();
        let w = Uuid::from_u128(1);
        let mut stats = CollectorStatistics::default();
        stats.record(w, snapshot(&[(v4, 4096)], &[]));

        // Collector restart: persist, then seed a fresh instance from the save point.
        let reseeded = CollectorStatistics::from_save_point(stats.save_point());
        assert_eq!(reseeded.total_bytes_for_ip[&v4], 4096);
        // The live view starts empty; workers reconnect from zero and accumulate on top of the seed.
        assert!(reseeded.latest_per_worker.is_empty());
    }
}
