//! Generic resource model (ARCHITECTURE.md §18).
//!
//! Kinds are discovered at runtime (no hardcoded enum). Each resource is
//! normalized from a `DynamicObject` into a `ResourceRow`. Well-known kinds get
//! hand-written status mappers + columns; everything else (including CRDs)
//! falls back to a generic mapper.

use k8s_openapi::jiff::Timestamp;
use serde_json::Value;

/// Metadata for a discovered kind.
#[derive(Debug, Clone, PartialEq)]
pub struct KindMeta {
    pub group: String,
    pub version: String,
    /// Kind name, e.g. "Deployment".
    pub kind: String,
    /// Lowercase plural, e.g. "deployments".
    pub plural: String,
    pub namespaced: bool,
}

impl KindMeta {
    /// Stable unique id: plural (+ group for non-core kinds).
    pub fn id(&self) -> String {
        if self.group.is_empty() {
            self.plural.clone()
        } else {
            format!("{}.{}", self.plural, self.group)
        }
    }

    /// Display label: Title-cased plural.
    pub fn label(&self) -> String {
        let mut chars = self.plural.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => self.kind.clone(),
        }
    }

    /// Left-nav category (design IA §6); unknown kinds → Custom Resources.
    pub fn category(&self) -> &'static str {
        category_for(&self.kind, &self.group)
    }
}

pub fn category_for(kind: &str, group: &str) -> &'static str {
    match kind {
        "Pod" | "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" | "CronJob" => {
            "Workloads"
        }
        "Service" | "Ingress" | "Endpoints" | "NetworkPolicy" | "IngressClass" => "Network",
        "ConfigMap" | "Secret" => "Config",
        "PersistentVolumeClaim" | "PersistentVolume" | "StorageClass" => "Storage",
        "Node" => "Nodes",
        "Event" => "Events",
        "Namespace" | "ServiceAccount" | "ResourceQuota" | "LimitRange" => "Cluster",
        _ if group.is_empty() || group.ends_with("k8s.io") => "Other",
        _ => "Custom Resources",
    }
}

/// Map a CRD's API group to a known project/operator icon key. The group is the
/// stable identity (e.g. every `*.strimzi.io` kind belongs to Strimzi), so one
/// logo covers all of a project's CRDs. Returns `None` for unrecognized groups
/// (those fall back to a tinted generic glyph via [`cr_tint`]).
pub fn cr_icon_key(group: &str) -> Option<&'static str> {
    let g = group;
    let key = if g == "monitoring.coreos.com" {
        "prometheus"
    } else if g.ends_with("strimzi.io") {
        "strimzi"
    } else if g.ends_with("k8s.elastic.co") {
        "elastic"
    } else if g.ends_with("external-secrets.io") {
        "external-secrets"
    } else if g.ends_with("kyverno.io") {
        "kyverno"
    } else if g.ends_with("envoyproxy.io") {
        "envoy"
    } else if g.starts_with("gateway.networking.") {
        "gateway-api"
    } else if g.ends_with("cert-manager.io") {
        "cert-manager"
    } else if g == "argoproj.io" {
        "argo"
    } else if g.starts_with("karpenter.") {
        "karpenter"
    } else if g == "opentelemetry.io" {
        "opentelemetry"
    } else if g.ends_with("k8ssandra.io") || g.ends_with("datastax.com") || g.contains("cassandra") {
        "cassandra"
    } else if g == "k6.io" {
        "k6"
    } else if g.ends_with("k8s.aws") || g.ends_with("k8s.amazonaws.com") {
        "aws"
    } else if g == "externaldns.k8s.io" {
        "external-dns"
    } else if g.ends_with("snapshot.storage.k8s.io")
        || g == "autoscaling.k8s.io"
        || g.ends_with("x-k8s.io")
    {
        "kubernetes"
    } else {
        return None;
    };
    Some(key)
}

/// Stable palette index (0..6) for a CRD group with no known logo, so different
/// operators get visually distinct tinted fallback glyphs.
pub fn cr_tint(group: &str) -> usize {
    let sum: u32 = group.bytes().map(|b| b as u32).sum();
    (sum % 6) as usize
}

/// One-line description of a well-known kind, shown under the page title.
/// Returns `None` for kinds we don't have a blurb for (e.g. arbitrary CRDs).
pub fn kind_description(kind: &str) -> Option<&'static str> {
    Some(match kind {
        // Workloads
        "Pod" => "The smallest deployable unit — one or more containers that run together.",
        "Deployment" => "Manages a replicated, self-healing set of Pods and rolls out updates.",
        "StatefulSet" => "Manages Pods with stable network identities and persistent storage.",
        "DaemonSet" => "Runs a copy of a Pod on every (or selected) node.",
        "ReplicaSet" => "Keeps a stable number of replica Pods running (usually owned by a Deployment).",
        "Job" => "Runs Pods to completion for a finite, one-off task.",
        "CronJob" => "Creates Jobs on a repeating schedule.",
        // Network
        "Service" => "A stable network endpoint that load-balances across a set of Pods.",
        "Ingress" => "HTTP/HTTPS routing rules from outside the cluster to Services.",
        "IngressClass" => "Selects which controller implements a set of Ingresses.",
        "Endpoints" => "The backing Pod IPs and ports a Service routes to.",
        "EndpointSlice" => "Scalable grouping of the network endpoints behind a Service.",
        "NetworkPolicy" => "Firewall rules controlling allowed Pod-to-Pod traffic.",
        // Config
        "ConfigMap" => "Non-confidential configuration data injected into Pods.",
        "Secret" => "Sensitive data (tokens, keys, certs) made available to Pods.",
        // Storage
        "PersistentVolumeClaim" => "A request for storage that binds to a PersistentVolume.",
        "PersistentVolume" => "A piece of cluster storage provisioned for use by Pods.",
        "StorageClass" => "Describes a class of storage and how volumes are dynamically provisioned.",
        "VolumeAttachment" => "Tracks attaching a volume to a node.",
        "CSIDriver" => "Registers a Container Storage Interface driver with the cluster.",
        "CSINode" => "Per-node information published by CSI drivers.",
        // Cluster
        "Node" => "A worker machine in the cluster that runs Pods.",
        "Namespace" => "A virtual cluster used to scope and isolate resources.",
        "ServiceAccount" => "An identity for processes running inside Pods.",
        "ResourceQuota" => "Caps aggregate resource usage within a namespace.",
        "LimitRange" => "Default and limit constraints for resources in a namespace.",
        "Event" => "A time-stamped record of something that happened to an object.",
        "PriorityClass" => "Defines a scheduling priority that Pods can reference.",
        "RuntimeClass" => "Selects the container runtime configuration for Pods.",
        "PodDisruptionBudget" => "Limits how many Pods can be voluntarily disrupted at once.",
        "HorizontalPodAutoscaler" => "Scales a workload's replicas based on observed metrics.",
        // RBAC
        "Role" => "A namespaced set of RBAC permissions.",
        "ClusterRole" => "A cluster-wide set of RBAC permissions.",
        "RoleBinding" => "Grants a Role to users, groups, or ServiceAccounts in a namespace.",
        "ClusterRoleBinding" => "Grants a ClusterRole cluster-wide.",
        // Admission / API extension
        "CustomResourceDefinition" => "Defines a new custom resource type served by the API.",
        "APIService" => "Registers an API group/version (often an aggregated API) with the API server.",
        "MutatingWebhookConfiguration" => "Admission webhooks that can modify API requests before they're stored.",
        "ValidatingWebhookConfiguration" => "Admission webhooks that accept or reject API requests.",
        "FlowSchema" => "Classifies API requests for priority and fairness.",
        "PriorityLevelConfiguration" => "Defines a concurrency limit for API request fairness.",
        "ComponentStatus" => "Health of cluster control-plane components (deprecated).",
        _ => return None,
    })
}

/// Kind-specific column headers (besides Name / Namespace / Status / Age).
pub fn columns_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "Pod" => &["Containers", "Restarts"],
        "Deployment" | "StatefulSet" | "ReplicaSet" => &["Ready", "Up-to-date", "Available"],
        "DaemonSet" => &["Desired", "Ready", "Available"],
        "Job" => &["Completions", "Duration"],
        "CronJob" => &["Schedule", "Active"],
        "Service" => &["Type", "Cluster-IP", "Ports"],
        "Ingress" => &["Class", "Hosts"],
        "ConfigMap" | "Secret" => &["Keys"],
        "PersistentVolumeClaim" => &["Phase", "Capacity"],
        "PersistentVolume" => &["Capacity", "Phase"],
        "Node" => &["Roles", "Version"],
        _ => &[],
    }
}

/// Whether this kind streams logs (Logs tab).
pub fn has_logs(kind: &str) -> bool {
    matches!(kind, "Pod" | "Deployment" | "StatefulSet" | "DaemonSet" | "Job")
}

/// Whether this kind is a key/value data resource (Data tab).
pub fn is_data_kind(kind: &str) -> bool {
    matches!(kind, "ConfigMap" | "Secret")
}

/// Whether this kind can be scaled / rollout-restarted.
pub fn is_workload(kind: &str) -> bool {
    matches!(kind, "Deployment" | "StatefulSet" | "ReplicaSet")
}

/// Per-container state for a pod (drives the Lens-style container squares).
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerState {
    pub name: String,
    /// Semantic class: running | pending | failed | neutral.
    pub class: String,
    pub ready: bool,
    pub restarts: i64,
    /// Human state: Running / CrashLoopBackOff / Completed / etc.
    pub state: String,
    /// "init" | "main".
    pub kind: String,
}

/// A normalized row the UI renders. Cheap to clone.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceRow {
    pub namespace: String,
    pub name: String,
    pub status: String,
    pub status_class: String,
    pub cols: Vec<String>,
    pub age: String,
    /// Per-container states (Pods only; empty otherwise).
    pub containers: Vec<ContainerState>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnState {
    Connecting,
    Live,
    Error(String),
}

/// A delta pushed from the engine to the UI over a channel.
#[derive(Debug, Clone)]
pub enum Delta {
    Context(String),
    /// The discovered kind catalog (sent once after connect).
    Catalog(Vec<KindMeta>),
    /// Resource created/updated, tagged with its kind id.
    Applied { kind_id: String, row: ResourceRow },
    Deleted {
        kind_id: String,
        namespace: String,
        name: String,
    },
    Reset,
    Conn(ConnState),

    Manifest {
        namespace: String,
        name: String,
        yaml: String,
    },
    ManifestErr {
        namespace: String,
        name: String,
        error: String,
    },
    LogReset,
    LogLine {
        source: String,
        idx: u8,
        line: String,
    },
    LogEnd,
    ActionResult {
        ok: bool,
        message: String,
    },
    /// Latest resource usage samples (from metrics.k8s.io).
    Metrics(Vec<MetricSample>),
    /// Current set of active port-forwards.
    PortForwards(Vec<PortForward>),

    /// Aggregated cluster overview snapshot.
    Overview(OverviewData),
    /// Events for the object shown in the detail panel (newest first).
    Events(Vec<EventRow>),
    /// Pods owned by the controller shown in the detail panel.
    ControllerPods(Vec<ResourceRow>),
    /// The connection lacks cluster-wide list access; the engine fell back to
    /// watching this single namespace (from the kubeconfig context).
    ScopedNamespace(String),
    /// The cluster's namespace list (for the namespace filter dropdown), kept
    /// independent of the active kind — cluster-scoped views (Nodes, PVs) carry
    /// no namespace and would otherwise leave the picker empty.
    Namespaces(Vec<String>),
    /// Full discovery (CRDs + everything) finished — the catalog is now complete.
    /// Lets the UI stop the "discovering" shimmer on the Custom Resources nav.
    DiscoveryComplete,

    // ---- Exec / terminal ----
    /// A new exec session started — clear the terminal.
    ExecReset,
    /// Terminal output (stdout/stderr from the exec'd process).
    ExecData(String),
    /// The exec session ended.
    ExecEnd,
}

/// CPU (millicores) + memory (bytes) usage for one object.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricSample {
    pub namespace: String,
    pub name: String,
    pub cpu_milli: i64,
    pub mem_bytes: i64,
}

/// Kinds that show live CPU/Mem columns.
pub fn has_metrics(kind: &str) -> bool {
    matches!(kind, "Pod" | "Node")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeUsage {
    pub name: String,
    pub ready: bool,
    pub cpu_pct: u8,
    pub mem_pct: u8,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EventRow {
    pub warn: bool,
    pub reason: String,
    pub object: String,
    pub message: String,
    pub age: String,
}

/// Aggregated cluster snapshot for the Overview dashboard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OverviewData {
    pub version: String,
    pub namespaces: usize,
    pub nodes_total: usize,
    pub nodes_ready: usize,
    pub pods_total: usize,
    pub pods_running: usize,
    pub pods_pending: usize,
    pub pods_failed: usize,
    pub pods_succeeded: usize,
    pub deployments_total: usize,
    pub deployments_available: usize,
    pub services_total: usize,
    pub services_lb: usize,
    pub cpu_used_milli: i64,
    pub cpu_cap_milli: i64,
    pub mem_used_bytes: i64,
    pub mem_cap_bytes: i64,
    pub nodes: Vec<NodeUsage>,
    pub events: Vec<EventRow>,
    // Per-section loaded flags (for progressive skeletons).
    pub ver_loaded: bool,
    pub ns_loaded: bool,
    pub nodes_loaded: bool,
    pub pods_loaded: bool,
    pub deps_loaded: bool,
    pub svcs_loaded: bool,
    pub events_loaded: bool,
}

impl OverviewData {
    /// Stale-while-revalidate merge for live refresh: start from `self` (the
    /// prior snapshot) and overwrite only the sections `incoming` has loaded, so
    /// a refresh updates values in place without flashing skeletons.
    pub fn merge_from(&self, incoming: &OverviewData) -> OverviewData {
        let mut out = self.clone();
        if incoming.ver_loaded {
            out.version = incoming.version.clone();
            out.ver_loaded = true;
        }
        if incoming.ns_loaded {
            out.namespaces = incoming.namespaces;
            out.ns_loaded = true;
        }
        if incoming.nodes_loaded {
            out.nodes_total = incoming.nodes_total;
            out.nodes_ready = incoming.nodes_ready;
            out.cpu_used_milli = incoming.cpu_used_milli;
            out.cpu_cap_milli = incoming.cpu_cap_milli;
            out.mem_used_bytes = incoming.mem_used_bytes;
            out.mem_cap_bytes = incoming.mem_cap_bytes;
            out.nodes = incoming.nodes.clone();
            out.nodes_loaded = true;
        }
        if incoming.pods_loaded {
            out.pods_total = incoming.pods_total;
            out.pods_running = incoming.pods_running;
            out.pods_pending = incoming.pods_pending;
            out.pods_failed = incoming.pods_failed;
            out.pods_succeeded = incoming.pods_succeeded;
            out.pods_loaded = true;
        }
        if incoming.deps_loaded {
            out.deployments_total = incoming.deployments_total;
            out.deployments_available = incoming.deployments_available;
            out.deps_loaded = true;
        }
        if incoming.svcs_loaded {
            out.services_total = incoming.services_total;
            out.services_lb = incoming.services_lb;
            out.svcs_loaded = true;
        }
        if incoming.events_loaded {
            out.events = incoming.events.clone();
            out.events_loaded = true;
        }
        out
    }
}

/// Build an `EventRow` from raw event fields (`warn` = type "Warning").
pub fn event_row(
    type_: Option<&str>,
    reason: Option<&str>,
    object: Option<&str>,
    message: Option<&str>,
    last_ts: Option<&str>,
) -> EventRow {
    EventRow {
        warn: type_ == Some("Warning"),
        reason: reason.unwrap_or_default().to_string(),
        object: object.unwrap_or_default().to_string(),
        message: message.unwrap_or_default().trim().to_string(),
        age: last_ts.map(age_of).unwrap_or_default(),
    }
}

/// Public timestamp→age helper (RFC3339 string).
pub fn age_of(ts: &str) -> String {
    match ts.parse::<Timestamp>() {
        Ok(t) => {
            let secs = Timestamp::now().duration_since(t).as_secs().max(0);
            match secs {
                s if s < 60 => format!("{s}s"),
                s if s < 3600 => format!("{}m", s / 60),
                s if s < 86_400 => format!("{}h", s / 3600),
                s => format!("{}d", s / 86_400),
            }
        }
        Err(_) => "-".into(),
    }
}

/// A command sent from the UI to the engine (UI → core). `kind_id` is a
/// `KindMeta::id()` resolved against the discovered registry.
#[derive(Debug, Clone)]
pub enum Cmd {
    SetKind(String),
    SetContext(String),
    FetchManifest {
        kind_id: String,
        namespace: String,
        name: String,
    },
    StartLogs {
        kind_id: String,
        namespace: String,
        name: String,
        container: Option<String>,
    },
    StopLogs,
    Delete {
        kind_id: String,
        namespace: String,
        name: String,
        /// Force delete: grace period 0 (skip graceful termination).
        force: bool,
    },
    Restart {
        kind_id: String,
        namespace: String,
        name: String,
    },
    Scale {
        kind_id: String,
        namespace: String,
        name: String,
        replicas: i32,
    },
    Apply {
        kind_id: String,
        namespace: String,
        name: String,
        yaml: String,
    },

    /// Open an interactive shell in a pod container.
    StartExec {
        namespace: String,
        name: String,
        container: Option<String>,
    },
    /// Send keystrokes to the active exec session's stdin.
    ExecInput(String),
    /// Resize the exec pty (terminal columns × rows).
    ExecResize { cols: u16, rows: u16 },
    /// Close the active exec session.
    StopExec,
    /// Fetch the cluster overview snapshot.
    FetchOverview,
    /// Fetch events for a single object (detail panel Events tab).
    FetchEvents { namespace: String, name: String },
    /// Set the namespace the watches are scoped to (None = all namespaces).
    /// Watching a single namespace is far cheaper than cluster-wide on big
    /// clusters (e.g. listing every Secret across all namespaces).
    SetNamespace(Option<String>),
    /// Force a full reconnect: rebuild the client from kubeconfig (re-running
    /// exec auth plugins so expired creds — e.g. after an `aws sso login` — are
    /// picked up), then restart the active watch. The in-watch self-heal only
    /// refreshes its own local client; this refreshes the engine's shared one,
    /// matching what relaunching the app does.
    Reconnect,
    /// Fetch the pods owned by a controller (Deployment/StatefulSet/…) for its
    /// detail panel.
    FetchControllerPods { kind_id: String, namespace: String, name: String },
    /// Enable/disable the background metrics poller (skip when unused).
    SetMetrics(bool),
    /// Stream merged logs across multiple pods (namespace, name).
    StartLogsPods(Vec<(String, String)>),
    /// Cordon (`on=true`) or uncordon a node.
    Cordon { name: String, on: bool },
    /// Cordon + evict pods from a node.
    Drain { name: String },
    /// Forward a local TCP port to a pod port.
    StartPortForward {
        namespace: String,
        name: String,
        pod_port: u16,
        local_port: u16,
    },
    /// Stop a forward by its local port.
    StopPortForward { local_port: u16 },
}

/// An active port-forward.
#[derive(Debug, Clone, PartialEq)]
pub struct PortForward {
    pub local_port: u16,
    pub namespace: String,
    pub name: String,
    pub pod_port: u16,
}

/// Normalize a resource (given its metadata + raw spec/status) into a row.
pub fn normalize(
    kind: &str,
    namespace: String,
    name: String,
    creation_ts: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>,
    data: &Value,
) -> ResourceRow {
    let age = age_str(creation_ts);
    match kind {
        "Pod" => map_pod(namespace, name, age, data),
        "Deployment" | "StatefulSet" | "ReplicaSet" => map_deployment(namespace, name, age, data),
        "DaemonSet" => map_daemonset(namespace, name, age, data),
        "Service" => map_service(namespace, name, age, data),
        "ConfigMap" | "Secret" => map_data_kind(kind, namespace, name, age, data),
        "Node" => map_node(namespace, name, age, data),
        "Job" => map_job(namespace, name, age, data),
        "CronJob" => map_cronjob(namespace, name, age, data),
        "Ingress" => map_ingress(namespace, name, age, data),
        "PersistentVolumeClaim" => map_pvc(namespace, name, age, data),
        "PersistentVolume" => map_pv(namespace, name, age, data),
        _ => map_generic(kind, namespace, name, age, data),
    }
}

fn parse_ts(s: &str) -> Option<Timestamp> {
    s.parse::<Timestamp>().ok()
}

fn fmt_dur(secs: i64) -> String {
    match secs {
        s if s < 0 => "-".into(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{}s", s / 60, s % 60),
        s if s < 86_400 => format!("{}h{}m", s / 3600, (s % 3600) / 60),
        s => format!("{}d{}h", s / 86_400, (s % 86_400) / 3600),
    }
}

fn map_job(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let desired = data["spec"]["completions"].as_i64().unwrap_or(1);
    let succeeded = data["status"]["succeeded"].as_i64().unwrap_or(0);
    let failed = data["status"]["failed"].as_i64().unwrap_or(0);
    let (status, class): (String, &str) = if succeeded >= desired && desired > 0 {
        ("Complete".into(), "running")
    } else if failed > 0 {
        ("Failed".into(), "failed")
    } else {
        ("Running".into(), "pending")
    };
    let start = data["status"]["startTime"].as_str().and_then(parse_ts);
    let end = data["status"]["completionTime"].as_str().and_then(parse_ts);
    let duration = match start {
        Some(s) => {
            let e = end.unwrap_or_else(Timestamp::now);
            fmt_dur(e.duration_since(s).as_secs())
        }
        None => "-".into(),
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class: class.into(),
        cols: vec![format!("{succeeded}/{desired}"), duration],
        age,
        containers: Vec::new(),
    }
}

fn map_cronjob(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let schedule = data["spec"]["schedule"].as_str().unwrap_or("-").to_string();
    let suspended = data["spec"]["suspend"].as_bool().unwrap_or(false);
    let active = data["status"]["active"].as_array().map(|a| a.len()).unwrap_or(0);
    let (status, class) = if suspended {
        ("Suspended".to_string(), "neutral")
    } else {
        ("Active".to_string(), "running")
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class: class.into(),
        cols: vec![schedule, active.to_string()],
        age,
        containers: Vec::new(),
    }
}

fn map_ingress(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let class = data["spec"]["ingressClassName"].as_str().unwrap_or("-").to_string();
    let hosts = data["spec"]["rules"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .filter_map(|r| r["host"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "*".into());
    let has_lb = data["status"]["loadBalancer"]["ingress"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    let (status, sclass) = if has_lb {
        ("Ready".to_string(), "running")
    } else {
        ("Pending".to_string(), "pending")
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class: sclass.into(),
        cols: vec![class, hosts],
        age,
        containers: Vec::new(),
    }
}

fn pvc_capacity(data: &Value) -> String {
    data["status"]["capacity"]["storage"]
        .as_str()
        .or_else(|| data["spec"]["resources"]["requests"]["storage"].as_str())
        .unwrap_or("-")
        .to_string()
}

fn map_pvc(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let phase = data["status"]["phase"].as_str().unwrap_or("Unknown").to_string();
    ResourceRow {
        namespace,
        name,
        status_class: phase_class(&phase).into(),
        status: phase.clone(),
        cols: vec![phase, pvc_capacity(data)],
        age,
        containers: Vec::new(),
    }
}

fn map_pv(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let phase = data["status"]["phase"].as_str().unwrap_or("Unknown").to_string();
    let capacity = data["spec"]["capacity"]["storage"].as_str().unwrap_or("-").to_string();
    ResourceRow {
        namespace,
        name,
        status_class: phase_class(&phase).into(),
        status: phase.clone(),
        cols: vec![capacity, phase],
        age,
        containers: Vec::new(),
    }
}

fn map_pod(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let phase = data["status"]["phase"].as_str().unwrap_or("Unknown").to_string();
    let (ready, restarts) = match data["status"]["containerStatuses"].as_array() {
        Some(cs) => {
            let total = cs.len();
            let ready = cs.iter().filter(|c| c["ready"].as_bool() == Some(true)).count();
            let restarts: i64 = cs.iter().map(|c| c["restartCount"].as_i64().unwrap_or(0)).sum();
            (format!("{ready}/{total}"), restarts)
        }
        None => ("0/0".into(), 0),
    };
    let containers = pod_containers(data);
    ResourceRow {
        namespace,
        name,
        status_class: phase_class(&phase).into(),
        status: phase,
        cols: vec![ready, restarts.to_string()],
        age,
        containers,
    }
}

/// Parse per-container states from a manifest YAML (for the detail panel).
pub fn container_states_from_yaml(yaml: &str) -> Vec<ContainerState> {
    serde_yaml::from_str::<Value>(yaml)
        .ok()
        .map(|v| pod_containers(&v))
        .unwrap_or_default()
}

/// Init + main container states from a pod's `status` (init first).
fn pod_containers(data: &Value) -> Vec<ContainerState> {
    let map = |arr: &Value, kind: &str| -> Vec<ContainerState> {
        arr.as_array()
            .map(|cs| cs.iter().map(|c| container_state(c, kind)).collect())
            .unwrap_or_default()
    };
    let mut out = map(&data["status"]["initContainerStatuses"], "init");
    out.extend(map(&data["status"]["containerStatuses"], "main"));
    out
}

/// Map a `containerStatuses[]` entry to a `ContainerState`.
fn container_state(c: &Value, kind: &str) -> ContainerState {
    let name = c["name"].as_str().unwrap_or("").to_string();
    let ready = c["ready"].as_bool() == Some(true);
    let restarts = c["restartCount"].as_i64().unwrap_or(0);
    let st = &c["state"];
    let (class, state): (&str, String) = if st.get("running").is_some() {
        (if ready { "running" } else { "pending" }, "Running".into())
    } else if let Some(w) = st.get("waiting") {
        let reason = w["reason"].as_str().unwrap_or("Waiting").to_string();
        let class = if reason.contains("BackOff") || reason.contains("Err") || reason.contains("Invalid") {
            "failed"
        } else {
            "pending"
        };
        (class, reason)
    } else if let Some(t) = st.get("terminated") {
        let code = t["exitCode"].as_i64().unwrap_or(0);
        let reason = t["reason"].as_str().map(str::to_string).unwrap_or_else(|| "Terminated".into());
        if code == 0 { ("neutral", reason) } else { ("failed", reason) }
    } else {
        ("neutral", "Unknown".into())
    };
    ContainerState { name, class: class.into(), ready, restarts, state, kind: kind.into() }
}

fn phase_class(phase: &str) -> &'static str {
    match phase {
        "Running" | "Active" | "Bound" | "Ready" | "Available" => "running",
        "Pending" | "Progressing" | "ContainerCreating" => "pending",
        "Failed" | "Error" | "CrashLoopBackOff" | "Lost" => "failed",
        "Succeeded" | "Completed" | "Released" => "neutral",
        _ => "neutral",
    }
}

fn map_deployment(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let desired = data["spec"]["replicas"].as_i64().unwrap_or(0);
    let ready = data["status"]["readyReplicas"].as_i64().unwrap_or(0);
    let updated = data["status"]["updatedReplicas"].as_i64().unwrap_or(0);
    let available = data["status"]["availableReplicas"].as_i64().unwrap_or(0);
    let (status, status_class): (String, String) = if desired == 0 {
        ("Scaled to 0".into(), "neutral".into())
    } else if ready >= desired {
        ("Available".into(), "running".into())
    } else {
        ("Progressing".into(), "pending".into())
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class,
        cols: vec![format!("{ready}/{desired}"), updated.to_string(), available.to_string()],
        age,
        containers: Vec::new(),
    }
}

fn map_daemonset(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let desired = data["status"]["desiredNumberScheduled"].as_i64().unwrap_or(0);
    let ready = data["status"]["numberReady"].as_i64().unwrap_or(0);
    let available = data["status"]["numberAvailable"].as_i64().unwrap_or(0);
    let (status, class) = if ready >= desired && desired > 0 {
        ("Ready".to_string(), "running")
    } else {
        ("Progressing".to_string(), "pending")
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class: class.into(),
        cols: vec![desired.to_string(), ready.to_string(), available.to_string()],
        age,
        containers: Vec::new(),
    }
}

fn map_service(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let svc_type = data["spec"]["type"].as_str().unwrap_or("ClusterIP").to_string();
    let cluster_ip = data["spec"]["clusterIP"].as_str().unwrap_or("-").to_string();
    let ports = data["spec"]["ports"]
        .as_array()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| p["port"].as_i64())
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    ResourceRow {
        namespace,
        name,
        status: svc_type.clone(),
        status_class: "info".into(),
        cols: vec![svc_type, cluster_ip, ports],
        age,
        containers: Vec::new(),
    }
}

fn map_data_kind(kind: &str, namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let d = data["data"].as_object().map(|m| m.len()).unwrap_or(0);
    let bd = data["binaryData"].as_object().map(|m| m.len()).unwrap_or(0);
    let (status, class) = if kind == "Secret" {
        (data["type"].as_str().unwrap_or("Opaque").to_string(), "info")
    } else {
        ("ConfigMap".to_string(), "neutral")
    };
    ResourceRow {
        namespace,
        name,
        status,
        status_class: class.into(),
        cols: vec![(d + bd).to_string()],
        age,
        containers: Vec::new(),
    }
}

fn map_node(namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let ready = data["status"]["conditions"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["type"].as_str() == Some("Ready")))
        .and_then(|c| c["status"].as_str())
        .map(|s| s == "True")
        .unwrap_or(false);
    let version = data["status"]["nodeInfo"]["kubeletVersion"]
        .as_str()
        .unwrap_or("-")
        .to_string();
    let roles = data["metadata"]["labels"]
        .as_object()
        .map(|m| {
            m.keys()
                .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                .collect::<Vec<_>>()
                .join(",")
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "<none>".into());
    ResourceRow {
        namespace,
        name,
        status: if ready { "Ready".into() } else { "NotReady".into() },
        status_class: if ready { "running".into() } else { "failed".into() },
        cols: vec![roles, version],
        age,
        containers: Vec::new(),
    }
}

/// Generic fallback: status from `status.phase` or a Ready/Available condition.
fn map_generic(kind: &str, namespace: String, name: String, age: String, data: &Value) -> ResourceRow {
    let status = data["status"]["phase"]
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            data["status"]["conditions"].as_array().and_then(|cs| {
                cs.iter()
                    .find(|c| {
                        matches!(c["type"].as_str(), Some("Ready" | "Available"))
                            && c["status"].as_str() == Some("True")
                    })
                    .and_then(|c| c["type"].as_str())
                    .map(str::to_string)
            })
        })
        .unwrap_or_default();
    let status_class = if status.is_empty() {
        "neutral".to_string()
    } else {
        phase_class(&status).to_string()
    };
    // Fill kind-specific columns if we have headers but no specialized mapper.
    let cols = columns_for(kind).iter().map(|_| "-".to_string()).collect();
    ResourceRow { namespace, name, status, status_class, cols, age, containers: Vec::new() }
}

fn age_str(ts: Option<&k8s_openapi::apimachinery::pkg::apis::meta::v1::Time>) -> String {
    let Some(ts) = ts else { return "-".into() };
    let secs = Timestamp::now().duration_since(ts.0).as_secs().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

/// All context names from the default kubeconfig.
pub fn list_contexts() -> Vec<String> {
    kube::config::Kubeconfig::read()
        .map(|k| k.contexts.into_iter().map(|c| c.name).collect())
        .unwrap_or_default()
}

/// Stable per-cluster accent index (1..=6) hashed from the context name.
pub fn cluster_accent_index(context: &str) -> u8 {
    let sum: usize = context.bytes().map(|b| b as usize).sum();
    (sum % 6) as u8 + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta(group: &str, kind: &str, plural: &str) -> KindMeta {
        KindMeta {
            group: group.into(),
            version: "v1".into(),
            kind: kind.into(),
            plural: plural.into(),
            namespaced: true,
        }
    }

    #[test]
    fn kind_id_core_vs_grouped() {
        assert_eq!(meta("", "Pod", "pods").id(), "pods");
        assert_eq!(meta("apps", "Deployment", "deployments").id(), "deployments.apps");
    }

    #[test]
    fn kind_label_titlecases_plural() {
        assert_eq!(meta("apps", "Deployment", "deployments").label(), "Deployments");
        assert_eq!(meta("", "Pod", "pods").label(), "Pods");
        // empty plural falls back to the kind
        assert_eq!(meta("", "Weird", "").label(), "Weird");
    }

    #[test]
    fn category_routing() {
        assert_eq!(category_for("Pod", ""), "Workloads");
        assert_eq!(category_for("CronJob", "batch"), "Workloads");
        assert_eq!(category_for("Service", ""), "Network");
        assert_eq!(category_for("Secret", ""), "Config");
        assert_eq!(category_for("PersistentVolume", ""), "Storage");
        assert_eq!(category_for("Node", ""), "Nodes");
        assert_eq!(category_for("Event", ""), "Events");
        assert_eq!(category_for("Namespace", ""), "Cluster");
        // built-in API groups → Other; third-party CRDs → Custom Resources
        assert_eq!(category_for("CSIDriver", "storage.k8s.io"), "Other");
        assert_eq!(category_for("Certificate", "cert-manager.io"), "Custom Resources");
    }

    #[test]
    fn classifiers() {
        assert!(has_logs("Pod") && has_logs("Deployment") && !has_logs("ConfigMap"));
        assert!(is_data_kind("ConfigMap") && is_data_kind("Secret") && !is_data_kind("Pod"));
        assert!(is_workload("Deployment") && is_workload("StatefulSet") && !is_workload("Pod"));
        assert!(has_metrics("Pod") && has_metrics("Node") && !has_metrics("Service"));
    }

    #[test]
    fn columns_known_and_fallback() {
        assert_eq!(columns_for("Pod"), &["Containers", "Restarts"]);
        assert_eq!(columns_for("Node"), &["Roles", "Version"]);
        assert!(columns_for("SomeCRD").is_empty());
    }

    #[test]
    fn phase_class_buckets() {
        assert_eq!(phase_class("Running"), "running");
        assert_eq!(phase_class("Pending"), "pending");
        assert_eq!(phase_class("CrashLoopBackOff"), "failed");
        assert_eq!(phase_class("Completed"), "neutral");
        assert_eq!(phase_class("Whatever"), "neutral");
    }

    #[test]
    fn fmt_dur_boundaries() {
        assert_eq!(fmt_dur(-5), "-");
        assert_eq!(fmt_dur(45), "45s");
        assert_eq!(fmt_dur(90), "1m30s");
        assert_eq!(fmt_dur(3700), "1h1m");
        assert_eq!(fmt_dur(90_000), "1d1h");
    }

    #[test]
    fn cluster_accent_in_range_and_stable() {
        for ctx in ["eks-green-dev", "eks-green-prod", "minikube", ""] {
            let i = cluster_accent_index(ctx);
            assert!((1..=6).contains(&i), "{ctx} -> {i}");
        }
        assert_eq!(cluster_accent_index("eks-green-dev"), cluster_accent_index("eks-green-dev"));
    }

    #[test]
    fn container_state_running_ready_vs_notready() {
        let ready = json!({"name": "app", "ready": true, "restartCount": 0, "state": {"running": {}}});
        let cs = container_state(&ready, "main");
        assert_eq!((cs.class.as_str(), cs.state.as_str(), cs.ready), ("running", "Running", true));

        let notready = json!({"name": "app", "ready": false, "restartCount": 2, "state": {"running": {}}});
        let cs = container_state(&notready, "main");
        assert_eq!(cs.class, "pending");
        assert_eq!(cs.restarts, 2);
    }

    #[test]
    fn container_state_waiting_and_terminated() {
        let crash = json!({"name": "c", "state": {"waiting": {"reason": "CrashLoopBackOff"}}});
        assert_eq!(container_state(&crash, "main").class, "failed");

        let creating = json!({"name": "c", "state": {"waiting": {"reason": "ContainerCreating"}}});
        assert_eq!(container_state(&creating, "main").class, "pending");

        let ok = json!({"name": "c", "state": {"terminated": {"exitCode": 0, "reason": "Completed"}}});
        assert_eq!(container_state(&ok, "init").class, "neutral");

        let bad = json!({"name": "c", "state": {"terminated": {"exitCode": 1, "reason": "Error"}}});
        assert_eq!(container_state(&bad, "main").class, "failed");
    }

    #[test]
    fn pod_containers_init_before_main() {
        let data = json!({"status": {
            "initContainerStatuses": [{"name": "init-db", "state": {"terminated": {"exitCode": 0}}}],
            "containerStatuses": [{"name": "app", "ready": true, "state": {"running": {}}}],
        }});
        let cs = pod_containers(&data);
        assert_eq!(cs.len(), 2);
        assert_eq!((cs[0].name.as_str(), cs[0].kind.as_str()), ("init-db", "init"));
        assert_eq!((cs[1].name.as_str(), cs[1].kind.as_str()), ("app", "main"));
    }

    #[test]
    fn map_pod_ready_and_restarts() {
        let data = json!({"status": {
            "phase": "Running",
            "containerStatuses": [
                {"name": "a", "ready": true, "restartCount": 1, "state": {"running": {}}},
                {"name": "b", "ready": false, "restartCount": 3, "state": {"waiting": {"reason": "CrashLoopBackOff"}}},
            ],
        }});
        let row = normalize("Pod", "ns".into(), "p".into(), None, &data);
        assert_eq!(row.status, "Running");
        assert_eq!(row.status_class, "running");
        assert_eq!(row.cols, vec!["1/2".to_string(), "4".to_string()]);
        assert_eq!(row.containers.len(), 2);
        assert_eq!(row.age, "-"); // None creation ts
    }

    #[test]
    fn map_deployment_states() {
        let avail = json!({"spec": {"replicas": 3}, "status": {"readyReplicas": 3, "updatedReplicas": 3, "availableReplicas": 3}});
        let row = normalize("Deployment", "ns".into(), "d".into(), None, &avail);
        assert_eq!((row.status.as_str(), row.status_class.as_str()), ("Available", "running"));
        assert_eq!(row.cols, vec!["3/3", "3", "3"]);

        let prog = json!({"spec": {"replicas": 3}, "status": {"readyReplicas": 1}});
        assert_eq!(normalize("Deployment", "ns".into(), "d".into(), None, &prog).status, "Progressing");

        let zero = json!({"spec": {"replicas": 0}});
        assert_eq!(normalize("Deployment", "ns".into(), "d".into(), None, &zero).status, "Scaled to 0");
    }

    #[test]
    fn map_job_outcomes() {
        let done = json!({"spec": {"completions": 1}, "status": {"succeeded": 1}});
        let row = normalize("Job", "ns".into(), "j".into(), None, &done);
        assert_eq!((row.status.as_str(), row.status_class.as_str()), ("Complete", "running"));
        assert_eq!(row.cols[0], "1/1");

        let failed = json!({"spec": {"completions": 1}, "status": {"failed": 2}});
        assert_eq!(normalize("Job", "ns".into(), "j".into(), None, &failed).status, "Failed");
    }

    #[test]
    fn map_cronjob_suspended() {
        let suspended = json!({"spec": {"schedule": "*/5 * * * *", "suspend": true}});
        let row = normalize("CronJob", "ns".into(), "c".into(), None, &suspended);
        assert_eq!((row.status.as_str(), row.cols[0].as_str()), ("Suspended", "*/5 * * * *"));
    }

    #[test]
    fn map_node_ready_and_roles() {
        let data = json!({
            "metadata": {"labels": {"node-role.kubernetes.io/control-plane": "", "x": "y"}},
            "status": {
                "conditions": [{"type": "Ready", "status": "True"}],
                "nodeInfo": {"kubeletVersion": "v1.34.0"},
            },
        });
        let row = normalize("Node", "".into(), "n1".into(), None, &data);
        assert_eq!((row.status.as_str(), row.status_class.as_str()), ("Ready", "running"));
        assert_eq!(row.cols, vec!["control-plane", "v1.34.0"]);
    }

    #[test]
    fn map_data_kind_counts_keys() {
        let cm = json!({"data": {"a": "1", "b": "2"}});
        let row = normalize("ConfigMap", "ns".into(), "cm".into(), None, &cm);
        assert_eq!((row.status.as_str(), row.cols[0].as_str()), ("ConfigMap", "2"));

        let sec = json!({"type": "kubernetes.io/tls", "data": {"tls.crt": "x"}, "binaryData": {"b": "y"}});
        let row = normalize("Secret", "ns".into(), "s".into(), None, &sec);
        assert_eq!((row.status.as_str(), row.cols[0].as_str()), ("kubernetes.io/tls", "2"));
    }

    #[test]
    fn unknown_kind_uses_generic_mapper() {
        // status.phase wins
        let phased = json!({"status": {"phase": "Bound"}});
        let row = normalize("FooBar", "ns".into(), "f".into(), None, &phased);
        assert_eq!((row.status.as_str(), row.status_class.as_str()), ("Bound", "running"));

        // else a Ready/Available True condition
        let cond = json!({"status": {"conditions": [{"type": "Ready", "status": "True"}]}});
        assert_eq!(normalize("FooBar", "ns".into(), "f".into(), None, &cond).status, "Ready");

        // else empty + neutral
        let empty = json!({});
        let row = normalize("FooBar", "ns".into(), "f".into(), None, &empty);
        assert_eq!((row.status.as_str(), row.status_class.as_str()), ("", "neutral"));
    }

    #[test]
    fn container_states_from_yaml_parses() {
        let yaml = "status:\n  containerStatuses:\n  - name: app\n    ready: true\n    state:\n      running: {}\n";
        let cs = container_states_from_yaml(yaml);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].name, "app");
        assert_eq!(cs[0].class, "running");
        // garbage yaml → empty, no panic
        assert!(container_states_from_yaml("::not yaml::").is_empty() || container_states_from_yaml(":\n  - [").is_empty());
    }

    #[test]
    fn overview_merge_keeps_stale_for_unloaded() {
        let prev = OverviewData {
            version: "1.34".into(),
            ver_loaded: true,
            pods_total: 100,
            pods_running: 90,
            pods_loaded: true,
            ..Default::default()
        };
        // A refresh in progress: only version reloaded, pods not yet.
        let incoming = OverviewData {
            version: "1.35".into(),
            ver_loaded: true,
            pods_total: 0,
            pods_loaded: false,
            ..Default::default()
        };
        let merged = prev.merge_from(&incoming);
        assert_eq!(merged.version, "1.35"); // loaded section updated
        assert!(merged.pods_loaded); // stale section kept (no skeleton)
        assert_eq!(merged.pods_total, 100);
        assert_eq!(merged.pods_running, 90);
    }

    #[test]
    fn overview_merge_updates_loaded_section() {
        let prev = OverviewData { pods_total: 100, pods_loaded: true, ..Default::default() };
        let incoming = OverviewData { pods_total: 120, pods_running: 118, pods_loaded: true, ..Default::default() };
        let merged = prev.merge_from(&incoming);
        assert_eq!((merged.pods_total, merged.pods_running), (120, 118));
    }

    #[test]
    fn cr_icon_key_maps_groups() {
        assert_eq!(cr_icon_key("monitoring.coreos.com"), Some("prometheus"));
        assert_eq!(cr_icon_key("kafka.strimzi.io"), Some("strimzi"));
        assert_eq!(cr_icon_key("elasticsearch.k8s.elastic.co"), Some("elastic"));
        assert_eq!(cr_icon_key("cert-manager.io"), Some("cert-manager"));
        assert_eq!(cr_icon_key("acme.cert-manager.io"), Some("cert-manager"));
        assert_eq!(cr_icon_key("gateway.networking.k8s.io"), Some("gateway-api"));
        assert_eq!(cr_icon_key("karpenter.k8s.aws"), Some("karpenter")); // karpenter wins over aws
        assert_eq!(cr_icon_key("elbv2.k8s.aws"), Some("aws"));
        assert_eq!(cr_icon_key("totally.unknown.example.com"), None);
    }

    #[test]
    fn cr_tint_is_stable_and_bounded() {
        assert_eq!(cr_tint("foo.bar"), cr_tint("foo.bar"));
        assert!(cr_tint("anything.io") < 6);
    }

    #[test]
    fn kind_description_known_and_unknown() {
        assert!(kind_description("Node").unwrap().contains("worker machine"));
        assert!(kind_description("Deployment").is_some());
        assert!(kind_description("ClusterRole").is_some());
        assert_eq!(kind_description("SomeRandomCRD"), None);
    }

    #[test]
    fn event_row_warn_and_fields() {
        let w = event_row(Some("Warning"), Some("BackOff"), Some("pod/x"), Some("  failed  "), None);
        assert!(w.warn);
        assert_eq!((w.reason.as_str(), w.object.as_str(), w.message.as_str()), ("BackOff", "pod/x", "failed"));
        assert_eq!(w.age, ""); // None ts

        let n = event_row(Some("Normal"), Some("Pulled"), None, None, Some("not-a-ts"));
        assert!(!n.warn);
        assert_eq!(n.age, "-"); // invalid ts → "-"
    }

    #[test]
    fn age_of_invalid_and_past() {
        assert_eq!(age_of("not-a-timestamp"), "-");
        // a long-ago fixed date is always many days old
        assert!(age_of("2000-01-01T00:00:00Z").ends_with('d'));
    }
}
