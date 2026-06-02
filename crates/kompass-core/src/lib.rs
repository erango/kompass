//! kompass-core — UI-agnostic Kubernetes engine.
//!
//! Spike scope: connect + watch pods, emit normalized deltas. Generalizes to
//! the generic-first discovery/store engine in ARCHITECTURE.md §18.

pub mod model;
pub mod watch;

pub use model::{
    age_of, category_for, cluster_accent_index, columns_for, container_states_from_yaml, has_logs,
    has_metrics, is_data_kind, ContainerState,
    is_workload, list_contexts, Cmd, ConnState, Delta, EventRow, KindMeta, MetricSample, NodeUsage,
    OverviewData, PortForward, ResourceRow,
};
pub use watch::run_engine;
