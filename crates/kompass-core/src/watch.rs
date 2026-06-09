//! Engine (ARCHITECTURE.md §1, §12, §18).
//!
//! Discovers all kinds the cluster serves (built-ins + CRDs), watches the
//! active kind generically via `DynamicObject`, and serves manifest/log/write
//! commands by kind id. All cluster I/O runs on background tokio tasks.

use std::collections::HashMap;

use futures::{AsyncBufReadExt, StreamExt};
use k8s_openapi::api::apps::v1::{Deployment, ReplicaSet};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{
    ApiResource, DeleteParams, DynamicObject, ListParams, LogParams, Patch, PatchParams,
};
use kube::discovery::{Discovery, Scope};
use kube::runtime::watcher::{self, Config, Event};
use kube::{Api, Client, ResourceExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::model::{normalize, Cmd, ConnState, Delta, KindMeta, MetricSample};

/// A discovered kind: how to talk to it + its name (for specialization).
#[derive(Clone)]
struct KindEntry {
    ar: ApiResource,
    kind: String,
    namespaced: bool,
}

type Registry = HashMap<String, KindEntry>;

pub async fn run_engine(tx: UnboundedSender<Delta>, mut cmd_rx: UnboundedReceiver<Cmd>) {
    let _ = tx.send(Delta::Conn(ConnState::Connecting));

    // Track the active context so reconnects can rebuild the client with fresh
    // credentials (exec plugins like `aws eks get-token` expire).
    let mut ctx_name: Option<String> =
        kube::config::Kubeconfig::read().ok().and_then(|c| c.current_context);
    if let Some(n) = &ctx_name {
        let _ = tx.send(Delta::Context(n.clone()));
    }

    // Core built-in kinds, known without discovery — register + show instantly
    // so the default view paints immediately instead of waiting on a full
    // cluster discovery (slow on clusters with many CRDs / aggregated APIs).
    let mut registry = core_registry();
    let _ = tx.send(Delta::Catalog(catalog_from(&registry)));
    let mut active = "deployments.apps".to_string();
    // The namespace the UI is viewing (None = all). Watches scope to it when set
    // (cheaper than cluster-wide on big clusters). A forced `scope_ns` wins.
    let mut view_ns: Option<String> = None;

    // Connect, retrying instead of giving up. If the initial client build fails
    // (commonly: exec auth creds already expired at launch — `aws eks get-token`
    // exits non-zero), returning here would kill the engine: the command loop
    // never starts, so "Retry now" / auto-reconnect have nothing to talk to and
    // only relaunching recovers. Instead stay alive and retry — on a timer and
    // immediately on a reconnect-intent command (Retry, context switch).
    let mut client = loop {
        match rebuild_client(&ctx_name).await {
            Ok(c) => break c,
            Err(e) => {
                let _ = tx.send(Delta::Conn(ConnState::Error(e.to_string())));
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                    cmd = cmd_rx.recv() => match cmd {
                        None => return, // UI gone
                        Some(Cmd::SetContext(name)) => {
                            ctx_name = Some(name.clone());
                            let _ = tx.send(Delta::Context(name));
                        }
                        Some(Cmd::SetKind(id)) => active = id,
                        Some(Cmd::SetNamespace(ns)) => view_ns = ns,
                        _ => {} // Reconnect / anything else: just retry now
                    },
                }
                // Stay in Error (banner over last-known data) while retrying —
                // re-emitting Connecting here flips the table to a loader every
                // attempt, which looks jumpy on a try/fail/try loop.
            }
        }
    };

    // Namespace scope: None = cluster-wide; Some(ns) = the connection can only
    // list that one namespace (detected on connect, recomputed on context switch).
    let mut scope_ns = detect_scope(&client, &ctx_name).await;
    if let Some(ns) = &scope_ns {
        let _ = tx.send(Delta::ScopedNamespace(ns.clone()));
    }
    refresh_namespaces(&client, &tx, &scope_ns);

    let mut watch_task =
        start_watch(&client, &tx, &registry, &active, &ctx_name, &scope_ns.clone().or_else(|| view_ns.clone()));
    let mut log_task: Option<JoinHandle<()>> = None;
    // Metrics poller runs only while a view needs it (toggled by SetMetrics).
    let mut metrics_on = false;
    let mut metrics_task: Option<JoinHandle<()>> = None;
    // Dropping the input sender ends the exec session (which kills the kubectl
    // child); aborting the task wouldn't reap the PTY child, so we don't abort.
    let mut exec_input: Option<UnboundedSender<ExecMsg>> = None;
    // Active port-forwards: local_port → (task, metadata).
    let mut forwards: HashMap<u16, (JoinHandle<()>, crate::model::PortForward)> = HashMap::new();

    // Full discovery (CRDs + everything else); merge + resend the catalog.
    let full = discover(&client).await;
    for (id, entry) in full {
        registry.entry(id).or_insert(entry);
    }
    let _ = tx.send(Delta::Catalog(catalog_from(&registry)));
    let _ = tx.send(Delta::DiscoveryComplete);

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Cmd::SetKind(id) => {
                active = id;
                if let Some(h) = watch_task.take() {
                    h.abort();
                }
                let _ = tx.send(Delta::Reset);
                let _ = tx.send(Delta::Conn(ConnState::Connecting));
                watch_task = start_watch(&client, &tx, &registry, &active, &ctx_name, &scope_ns.clone().or_else(|| view_ns.clone()));
            }
            Cmd::SetNamespace(ns) => {
                if view_ns != ns {
                    view_ns = ns;
                    if let Some(h) = watch_task.take() {
                        h.abort();
                    }
                    let _ = tx.send(Delta::Reset);
                    let _ = tx.send(Delta::Conn(ConnState::Connecting));
                    watch_task = start_watch(&client, &tx, &registry, &active, &ctx_name, &scope_ns.clone().or_else(|| view_ns.clone()));
                }
            }
            Cmd::Reconnect => {
                if let Some(h) = watch_task.take() {
                    h.abort();
                }
                // Keep last-known rows visible (no Reset) while we rebuild the
                // shared client so re-run exec auth picks up fresh creds (the
                // in-watch self-heal only refreshes its local copy).
                let _ = tx.send(Delta::Conn(ConnState::Connecting));
                match rebuild_client(&ctx_name).await {
                    Ok(c) => {
                        client = c;
                        scope_ns = detect_scope(&client, &ctx_name).await;
                        if let Some(ns) = &scope_ns {
                            let _ = tx.send(Delta::ScopedNamespace(ns.clone()));
                        }
                        refresh_namespaces(&client, &tx, &scope_ns);
                    }
                    Err(e) => {
                        let _ = tx.send(Delta::Conn(ConnState::Error(e.to_string())));
                    }
                }
                watch_task = start_watch(&client, &tx, &registry, &active, &ctx_name, &scope_ns.clone().or_else(|| view_ns.clone()));
            }
            Cmd::SetContext(name) => {
                if let Some(h) = watch_task.take() {
                    h.abort();
                }
                if let Some(h) = log_task.take() {
                    h.abort();
                }
                if let Some(h) = metrics_task.take() {
                    h.abort();
                }
                exec_input = None;
                // Tear down port-forwards bound to the old cluster.
                for (_, (task, _)) in forwards.drain() {
                    task.abort();
                }
                emit_forwards(&tx, &forwards);
                let _ = tx.send(Delta::Reset);
                let _ = tx.send(Delta::Conn(ConnState::Connecting));
                match client_for_context(&name).await {
                    Ok(c) => {
                        client = c;
                        ctx_name = Some(name.clone());
                        let _ = tx.send(Delta::Context(name));
                        scope_ns = detect_scope(&client, &ctx_name).await;
                        if let Some(ns) = &scope_ns {
                            let _ = tx.send(Delta::ScopedNamespace(ns.clone()));
                        }
                        refresh_namespaces(&client, &tx, &scope_ns);
                        view_ns = None; // UI re-sends the new cluster's view ns
                        registry = core_registry();
                        let _ = tx.send(Delta::Catalog(catalog_from(&registry)));
                        watch_task = start_watch(&client, &tx, &registry, &active, &ctx_name, &scope_ns.clone().or_else(|| view_ns.clone()));
                        let full = discover(&client).await;
                        for (id, entry) in full {
                            registry.entry(id).or_insert(entry);
                        }
                        let _ = tx.send(Delta::Catalog(catalog_from(&registry)));
                        let _ = tx.send(Delta::DiscoveryComplete);
                        if metrics_on {
                            metrics_task = Some(tokio::spawn(poll_metrics(client.clone(), tx.clone())));
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Delta::Conn(ConnState::Error(e.to_string())));
                    }
                }
            }
            Cmd::FetchManifest { kind_id, namespace, name } => {
                if let Some(e) = registry.get(&kind_id).cloned() {
                    let (c, t) = (client.clone(), tx.clone());
                    tokio::spawn(async move { fetch_manifest(c, t, e, namespace, name).await });
                }
            }
            Cmd::StartLogs { kind_id, namespace, name, container } => {
                if let Some(h) = log_task.take() {
                    h.abort();
                }
                let _ = tx.send(Delta::LogReset);
                let kind = registry.get(&kind_id).map(|e| e.kind.clone()).unwrap_or_default();
                let (c, t) = (client.clone(), tx.clone());
                log_task = Some(tokio::spawn(async move {
                    if kind == "Pod" {
                        let api: Api<Pod> = Api::namespaced(c, &namespace);
                        // group_by_pod=false → color/label per container.
                        pod_all_logs(api, t, name, 0, container, false).await;
                    } else {
                        // Controller: stream across its pods (color per pod).
                        stream_controller_logs(c, t, namespace, name, container).await;
                    }
                }));
            }
            Cmd::StopLogs => {
                if let Some(h) = log_task.take() {
                    h.abort();
                }
            }
            Cmd::Delete { kind_id, namespace, name, force } => {
                if let Some(e) = registry.get(&kind_id).cloned() {
                    let (c, t) = (client.clone(), tx.clone());
                    tokio::spawn(async move { delete(c, t, e, namespace, name, force).await });
                }
            }
            Cmd::Restart { kind_id, namespace, name } => {
                let kind = registry.get(&kind_id).map(|e| e.kind.clone()).unwrap_or_default();
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { restart(c, t, kind, namespace, name).await });
            }
            Cmd::Scale { kind_id, namespace, name, replicas } => {
                let kind = registry.get(&kind_id).map(|e| e.kind.clone()).unwrap_or_default();
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { scale(c, t, kind, namespace, name, replicas).await });
            }
            Cmd::Apply { kind_id, namespace, name, yaml } => {
                if let Some(e) = registry.get(&kind_id).cloned() {
                    let (c, t) = (client.clone(), tx.clone());
                    tokio::spawn(async move { apply_yaml(c, t, e, namespace, name, yaml).await });
                }
            }
            Cmd::StartExec { namespace, name, container } => {
                // Drop any previous session (closes its in_rx → kills its child).
                let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<ExecMsg>();
                exec_input = Some(in_tx);
                let _ = tx.send(Delta::ExecReset);
                let (t, ctx) = (tx.clone(), ctx_name.clone());
                tokio::spawn(async move {
                    exec_session(t, ctx, namespace, name, container, in_rx).await
                });
            }
            Cmd::ExecInput(s) => {
                if let Some(tx) = &exec_input {
                    let _ = tx.send(ExecMsg::Stdin(s.into_bytes()));
                }
            }
            Cmd::ExecResize { cols, rows } => {
                if let Some(tx) = &exec_input {
                    let _ = tx.send(ExecMsg::Resize(cols, rows));
                }
            }
            Cmd::StopExec => {
                exec_input = None;
            }
            Cmd::FetchOverview => {
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { fetch_overview(c, t).await });
            }
            Cmd::FetchEvents { namespace, name } => {
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { fetch_events(c, namespace, name, t).await });
            }
            Cmd::FetchControllerPods { kind_id, namespace, name } => {
                if let Some(e) = registry.get(&kind_id).cloned() {
                    let (c, t) = (client.clone(), tx.clone());
                    tokio::spawn(async move { fetch_controller_pods(c, e, namespace, name, t).await });
                }
            }
            Cmd::SetMetrics(on) => {
                if on != metrics_on {
                    metrics_on = on;
                    if on {
                        metrics_task = Some(tokio::spawn(poll_metrics(client.clone(), tx.clone())));
                    } else if let Some(h) = metrics_task.take() {
                        h.abort();
                    }
                }
            }
            Cmd::Cordon { name, on } => {
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { cordon(c, t, name, on).await });
            }
            Cmd::Drain { name } => {
                let (c, t) = (client.clone(), tx.clone());
                tokio::spawn(async move { drain(c, t, name).await });
            }
            Cmd::StartPortForward { namespace, name, pod_port, local_port } => {
                if forwards.contains_key(&local_port) {
                    action_result(&tx, false, format!("Local port {local_port} already in use"));
                } else {
                    let (c, t) = (client.clone(), tx.clone());
                    let (ns, nm) = (namespace.clone(), name.clone());
                    let task = tokio::spawn(async move {
                        port_forward(c, t, ns, nm, pod_port, local_port).await
                    });
                    forwards.insert(
                        local_port,
                        (task, crate::model::PortForward { local_port, namespace, name, pod_port }),
                    );
                    emit_forwards(&tx, &forwards);
                }
            }
            Cmd::StopPortForward { local_port } => {
                if let Some((task, _)) = forwards.remove(&local_port) {
                    task.abort();
                    emit_forwards(&tx, &forwards);
                }
            }
            Cmd::StartLogsPods(pods) => {
                if let Some(h) = log_task.take() {
                    h.abort();
                }
                let _ = tx.send(Delta::LogReset);
                let (c, t) = (client.clone(), tx.clone());
                log_task = Some(tokio::spawn(async move {
                    let loops = pods.into_iter().enumerate().map(|(i, (ns, name))| {
                        let api: Api<Pod> = Api::namespaced(c.clone(), &ns);
                        let t = t.clone();
                        async move { pod_all_logs(api, t, name, (i % 8) as u8, None, true).await }
                    });
                    futures::future::join_all(loops).await;
                }));
            }
        }
    }
}

fn emit_forwards(
    tx: &UnboundedSender<Delta>,
    forwards: &HashMap<u16, (JoinHandle<()>, crate::model::PortForward)>,
) {
    let mut list: Vec<crate::model::PortForward> =
        forwards.values().map(|(_, pf)| pf.clone()).collect();
    list.sort_by_key(|p| p.local_port);
    let _ = tx.send(Delta::PortForwards(list));
}

async fn cordon(client: Client, tx: UnboundedSender<Delta>, name: String, on: bool) {
    use k8s_openapi::api::core::v1::Node;
    let api: Api<Node> = Api::all(client);
    let patch = serde_json::json!({ "spec": { "unschedulable": on } });
    match api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await {
        Ok(_) => action_result(
            &tx,
            true,
            if on { format!("Cordoned {name}") } else { format!("Uncordoned {name}") },
        ),
        Err(e) => action_result(&tx, false, format!("Cordon failed: {e}")),
    }
}

async fn drain(client: Client, tx: UnboundedSender<Delta>, name: String) {
    use k8s_openapi::api::core::v1::{Node, Pod};
    // Cordon first.
    let node_api: Api<Node> = Api::all(client.clone());
    let patch = serde_json::json!({ "spec": { "unschedulable": true } });
    if let Err(e) = node_api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch)).await {
        return action_result(&tx, false, format!("Drain failed (cordon): {e}"));
    }
    // Evict pods scheduled on the node (skip DaemonSet-managed + mirror pods).
    let pods: Api<Pod> = Api::all(client.clone());
    let lp = ListParams::default().fields(&format!("spec.nodeName={name}"));
    match pods.list(&lp).await {
        Ok(list) => {
            let mut n = 0;
            for p in list.items {
                let ds = p.owner_references().iter().any(|o| o.kind == "DaemonSet");
                let mirror = p.metadata.annotations.as_ref().is_some_and(|a| {
                    a.contains_key("kubernetes.io/config.mirror")
                });
                if ds || mirror {
                    continue;
                }
                let ns = p.namespace().unwrap_or_default();
                let pn = p.name_any();
                let _ = Api::<Pod>::namespaced(client.clone(), &ns)
                    .delete(&pn, &DeleteParams::default())
                    .await;
                n += 1;
            }
            action_result(&tx, true, format!("Drained {name}: evicted {n} pods"));
        }
        Err(e) => action_result(&tx, false, format!("Drain failed (list): {e}")),
    }
}

/// Bind a local TCP port and forward each connection to the pod's port.
async fn port_forward(
    client: Client,
    tx: UnboundedSender<Delta>,
    namespace: String,
    name: String,
    pod_port: u16,
    local_port: u16,
) {
    use k8s_openapi::api::core::v1::Pod;
    use tokio::net::TcpListener;

    let listener = match TcpListener::bind(("127.0.0.1", local_port)).await {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(Delta::ActionResult { ok: false, message: format!("Port {local_port} bind failed: {e}") });
            return;
        }
    };
    let _ = tx.send(Delta::ActionResult {
        ok: true,
        message: format!("Forwarding localhost:{local_port} → {name}:{pod_port}"),
    });
    let api: Api<Pod> = Api::namespaced(client, &namespace);
    loop {
        let Ok((mut conn, _)) = listener.accept().await else { break };
        let api = api.clone();
        let name = name.clone();
        tokio::spawn(async move {
            if let Ok(mut pf) = api.portforward(&name, &[pod_port]).await {
                if let Some(mut upstream) = pf.take_stream(pod_port) {
                    let _ = tokio::io::copy_bidirectional(&mut conn, &mut upstream).await;
                }
            }
        });
    }
}

/// Gather an aggregate cluster snapshot for the Overview dashboard.
async fn fetch_overview(client: Client, tx: UnboundedSender<Delta>) {
    use k8s_openapi::api::apps::v1::Deployment;
    use k8s_openapi::api::core::v1::{Event, Namespace, Node, Pod, Service};
    use std::sync::{Arc, Mutex};

    // Shared snapshot; each section fills its fields and emits progressively so
    // the dashboard paints as data arrives instead of blocking on the slowest call.
    let data = Arc::new(Mutex::new(crate::model::OverviewData::default()));
    let emit = |data: &Arc<Mutex<crate::model::OverviewData>>| {
        let _ = tx.send(Delta::Overview(data.lock().unwrap().clone()));
    };

    let version = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(v) = client.apiserver_version().await {
                let mut d = data.lock().unwrap();
                d.version = v.git_version;
                d.ver_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    let namespaces = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(l) = Api::<Namespace>::all(client).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                d.namespaces = l.items.len();
                d.ns_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    // Nodes (capacity + readiness) then node metrics → per-node % + totals.
    let nodes = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            let mut cap: std::collections::HashMap<String, (i64, i64)> = Default::default();
            if let Ok(l) = Api::<Node>::all(client.clone()).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                d.nodes_total = l.items.len();
                for n in l.items {
                    let name = n.metadata.name.clone().unwrap_or_default();
                    let ready = n.status.as_ref().and_then(|s| s.conditions.as_ref())
                        .map(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
                        .unwrap_or(false);
                    if ready { d.nodes_ready += 1; }
                    let (mut cc, mut mc) = (0i64, 0i64);
                    if let Some(c) = n.status.as_ref().and_then(|s| s.capacity.as_ref()) {
                        cc = c.get("cpu").map(|q| parse_cpu_milli(&q.0)).unwrap_or(0);
                        mc = c.get("memory").map(|q| parse_mem_bytes(&q.0)).unwrap_or(0);
                    }
                    d.cpu_cap_milli += cc;
                    d.mem_cap_bytes += mc;
                    cap.insert(name.clone(), (cc, mc));
                    d.nodes.push(crate::model::NodeUsage { name, ready, cpu_pct: 0, mem_pct: 0 });
                }
                d.nodes_loaded = true;
            }
            emit(&data);
            let node_ar = ApiResource {
                group: "metrics.k8s.io".into(), version: "v1beta1".into(),
                api_version: "metrics.k8s.io/v1beta1".into(),
                kind: "NodeMetrics".into(), plural: "nodes".into(),
            };
            if let Ok(l) = Api::<DynamicObject>::all_with(client, &node_ar).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                for o in l {
                    let name = o.metadata.name.clone().unwrap_or_default();
                    let cu = parse_cpu_milli(o.data["usage"]["cpu"].as_str().unwrap_or("0"));
                    let mu = parse_mem_bytes(o.data["usage"]["memory"].as_str().unwrap_or("0"));
                    d.cpu_used_milli += cu;
                    d.mem_used_bytes += mu;
                    if let (Some((cc, mc)), Some(nu)) =
                        (cap.get(&name).copied(), d.nodes.iter_mut().find(|n| n.name == name))
                    {
                        nu.cpu_pct = pct(cu, cc);
                        nu.mem_pct = pct(mu, mc);
                    }
                }
                drop(d);
                emit(&data);
            }
        }
    };

    let pods = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(l) = Api::<Pod>::all(client).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                d.pods_total = l.items.len();
                for p in l.items {
                    match p.status.and_then(|s| s.phase).as_deref() {
                        Some("Running") => d.pods_running += 1,
                        Some("Pending") => d.pods_pending += 1,
                        Some("Failed") => d.pods_failed += 1,
                        Some("Succeeded") => d.pods_succeeded += 1,
                        _ => {}
                    }
                }
                d.pods_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    let deploys = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(l) = Api::<Deployment>::all(client).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                d.deployments_total = l.items.len();
                for dep in l.items {
                    let desired = dep.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
                    let avail = dep.status.as_ref().and_then(|s| s.available_replicas).unwrap_or(0);
                    if avail >= desired { d.deployments_available += 1; }
                }
                d.deps_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    let services = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(l) = Api::<Service>::all(client).list(&Default::default()).await {
                let mut d = data.lock().unwrap();
                d.services_total = l.items.len();
                for s in l.items {
                    if s.spec.and_then(|sp| sp.type_).as_deref() == Some("LoadBalancer") {
                        d.services_lb += 1;
                    }
                }
                d.svcs_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    let events = {
        let (client, data, emit) = (client.clone(), data.clone(), &emit);
        async move {
            if let Ok(l) = Api::<Event>::all(client).list(&Default::default()).await {
                let mut evs: Vec<Event> = l.items.into_iter()
                    .filter(|e| e.type_.as_deref() == Some("Warning")).collect();
                evs.sort_by(|a, b| {
                    let ta = a.last_timestamp.as_ref().map(|t| t.0);
                    let tb = b.last_timestamp.as_ref().map(|t| t.0);
                    tb.cmp(&ta)
                });
                let mut d = data.lock().unwrap();
                for e in evs.into_iter().take(8) {
                    d.events.push(crate::model::EventRow {
                        warn: true,
                        reason: e.reason.unwrap_or_default(),
                        object: e.involved_object.name.unwrap_or_default(),
                        message: e.message.unwrap_or_default(),
                        age: e.last_timestamp.map(|t| crate::model::age_of(&t.0.to_string())).unwrap_or_default(),
                    });
                }
                d.events_loaded = true;
                drop(d);
                emit(&data);
            }
        }
    };

    // Run all sections concurrently; each emits as it lands.
    futures::join!(version, namespaces, nodes, pods, deploys, services, events);
}

/// List events for a single object (newest first) and emit them.
async fn fetch_events(client: Client, namespace: String, name: String, tx: UnboundedSender<Delta>) {
    use k8s_openapi::api::core::v1::Event as CoreEvent;
    let api: Api<CoreEvent> = Api::namespaced(client, &namespace);
    let lp = ListParams::default().fields(&format!("involvedObject.name={name}"));
    let rows = match api.list(&lp).await {
        Ok(l) => {
            let mut items = l.items;
            items.sort_by(|a, b| {
                let ta = a.last_timestamp.as_ref().map(|t| t.0);
                let tb = b.last_timestamp.as_ref().map(|t| t.0);
                tb.cmp(&ta) // newest first
            });
            items
                .into_iter()
                .map(|e| {
                    let ts = e.last_timestamp.as_ref().map(|t| t.0.to_string());
                    crate::model::event_row(
                        e.type_.as_deref(),
                        e.reason.as_deref(),
                        e.involved_object.name.as_deref(),
                        e.message.as_deref(),
                        ts.as_deref(),
                    )
                })
                .collect()
        }
        Err(_) => Vec::new(),
    };
    let _ = tx.send(Delta::Events(rows));
}

/// List the pods owned by a controller (via its `spec.selector.matchLabels`),
/// normalized into rows for the detail panel.
async fn fetch_controller_pods(
    client: Client,
    entry: KindEntry,
    namespace: String,
    name: String,
    tx: UnboundedSender<Delta>,
) {
    let ctrl: Api<DynamicObject> = Api::namespaced_with(client.clone(), &namespace, &entry.ar);
    let obj = match ctrl.get(&name).await {
        Ok(o) => o,
        Err(_) => {
            let _ = tx.send(Delta::ControllerPods(Vec::new()));
            return;
        }
    };
    let selector = obj.data["spec"]["selector"]["matchLabels"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    if selector.is_empty() {
        let _ = tx.send(Delta::ControllerPods(Vec::new()));
        return;
    }
    let pod_ar = kube::api::ApiResource::erase::<Pod>(&());
    let pods: Api<DynamicObject> = Api::namespaced_with(client, &namespace, &pod_ar);
    let rows = match pods.list(&ListParams::default().labels(&selector)).await {
        Ok(l) => l
            .items
            .into_iter()
            .map(|o| {
                normalize(
                    "Pod",
                    o.metadata.namespace.clone().unwrap_or_default(),
                    o.metadata.name.clone().unwrap_or_default(),
                    o.metadata.creation_timestamp.as_ref(),
                    &o.data,
                )
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let _ = tx.send(Delta::ControllerPods(rows));
}

fn pct(used: i64, cap: i64) -> u8 {
    if cap <= 0 {
        return 0;
    }
    ((used as f64 / cap as f64 * 100.0).round() as i64).clamp(0, 100) as u8
}

/// Built-in kinds known without discovery: (group, version, kind, plural, namespaced).
const CORE_KINDS: &[(&str, &str, &str, &str, bool)] = &[
    ("", "v1", "Pod", "pods", true),
    ("apps", "v1", "Deployment", "deployments", true),
    ("apps", "v1", "StatefulSet", "statefulsets", true),
    ("apps", "v1", "DaemonSet", "daemonsets", true),
    ("apps", "v1", "ReplicaSet", "replicasets", true),
    ("", "v1", "Service", "services", true),
    ("networking.k8s.io", "v1", "Ingress", "ingresses", true),
    ("", "v1", "ConfigMap", "configmaps", true),
    ("", "v1", "Secret", "secrets", true),
    ("batch", "v1", "Job", "jobs", true),
    ("batch", "v1", "CronJob", "cronjobs", true),
    ("", "v1", "PersistentVolumeClaim", "persistentvolumeclaims", true),
    ("", "v1", "PersistentVolume", "persistentvolumes", false),
    ("", "v1", "Node", "nodes", false),
    ("", "v1", "Namespace", "namespaces", false),
    ("", "v1", "Event", "events", true),
];

fn core_registry() -> Registry {
    let mut reg = Registry::new();
    for (group, version, kind, plural, namespaced) in CORE_KINDS.iter().copied() {
        let api_version = if group.is_empty() {
            version.to_string()
        } else {
            format!("{group}/{version}")
        };
        let ar = ApiResource {
            group: group.to_string(),
            version: version.to_string(),
            api_version,
            kind: kind.to_string(),
            plural: plural.to_string(),
        };
        let meta = KindMeta {
            group: group.to_string(),
            version: version.to_string(),
            kind: kind.to_string(),
            plural: plural.to_string(),
            namespaced,
        };
        reg.insert(meta.id(), KindEntry { ar, kind: kind.to_string(), namespaced });
    }
    reg
}

/// Build the UI catalog from a registry.
fn catalog_from(reg: &Registry) -> Vec<KindMeta> {
    let mut metas: Vec<KindMeta> = reg
        .iter()
        .map(|(_, e)| KindMeta {
            group: e.ar.group.clone(),
            version: e.ar.version.clone(),
            kind: e.kind.clone(),
            plural: e.ar.plural.clone(),
            namespaced: e.namespaced,
        })
        .collect();
    metas.sort_by(|a, b| a.kind.cmp(&b.kind));
    metas
}

/// Run full discovery and return the kind registry (no catalog send — the
/// caller merges with the core set and emits the authoritative catalog).
async fn discover(client: &Client) -> Registry {
    let mut reg: Registry = HashMap::new();
    if let Ok(disc) = Discovery::new(client.clone()).run().await {
        for group in disc.groups() {
            for (ar, caps) in group.recommended_resources() {
                // `events.k8s.io/v1 Event` duplicates core `v1 Event` — keep the
                // core one (already registered) so the nav shows a single Events.
                if ar.group == "events.k8s.io" && ar.kind == "Event" {
                    continue;
                }
                // metrics.k8s.io NodeMetrics/PodMetrics share the plurals
                // "nodes"/"pods", so they show up as duplicate Nodes/Pods items.
                // They're raw metrics (consumed by the poller), not browsable.
                if ar.group == "metrics.k8s.io" {
                    continue;
                }
                let namespaced = caps.scope == Scope::Namespaced;
                let id = KindMeta {
                    group: ar.group.clone(),
                    version: ar.version.clone(),
                    kind: ar.kind.clone(),
                    plural: ar.plural.clone(),
                    namespaced,
                }
                .id();
                reg.entry(id)
                    .or_insert(KindEntry { ar: ar.clone(), kind: ar.kind.clone(), namespaced });
            }
        }
    }
    reg
}

fn start_watch(
    client: &Client,
    tx: &UnboundedSender<Delta>,
    registry: &Registry,
    id: &str,
    ctx: &Option<String>,
    scope: &Option<String>,
) -> Option<JoinHandle<()>> {
    let entry = registry.get(id)?.clone();
    let kind_id = id.to_string();
    Some(tokio::spawn(watch_kind(
        client.clone(),
        tx.clone(),
        entry,
        kind_id,
        ctx.clone(),
        scope.clone(),
    )))
}

/// The namespace set on a kubeconfig context (the namespace a scoped user can list).
fn context_namespace(ctx: &Option<String>) -> Option<String> {
    let name = ctx.as_ref()?;
    let kc = kube::config::Kubeconfig::read().ok()?;
    kc.contexts
        .into_iter()
        .find(|c| &c.name == name)
        .and_then(|c| c.context)
        .and_then(|c| c.namespace)
}

/// Detect a namespace-scoped connection: if a cluster-wide pod list is forbidden
/// and the context names a namespace, that namespace is what we *can* list.
async fn detect_scope(client: &Client, ctx: &Option<String>) -> Option<String> {
    let ctx_ns = context_namespace(ctx)?;
    let ar = kube::api::ApiResource::erase::<k8s_openapi::api::core::v1::Pod>(&());
    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    match api.list(&ListParams::default().limit(1)).await {
        Err(kube::Error::Api(e)) if e.code == 403 => Some(ctx_ns),
        _ => None,
    }
}

/// Fetch the cluster's namespace list for the filter dropdown and emit it.
/// Spawned so it never blocks the connect path. On a namespace-scoped
/// connection (cluster-wide list forbidden) it reports just the scoped one.
fn refresh_namespaces(client: &Client, tx: &UnboundedSender<Delta>, scope: &Option<String>) {
    let client = client.clone();
    let tx = tx.clone();
    let scope = scope.clone();
    tokio::spawn(async move {
        use k8s_openapi::api::core::v1::Namespace;
        let list = match Api::<Namespace>::all(client).list(&ListParams::default()).await {
            Ok(l) => {
                let mut v: Vec<String> = l.items.into_iter().filter_map(|n| n.metadata.name).collect();
                v.sort();
                v
            }
            Err(_) => scope.clone().into_iter().collect(),
        };
        let _ = tx.send(Delta::Namespaces(list));
    });
}

/// Rebuild a client (fresh credentials) for the active context.
async fn rebuild_client(ctx: &Option<String>) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    match ctx {
        Some(name) => client_for_context(name).await,
        None => Ok(Client::try_default().await?),
    }
}

async fn watch_kind(
    mut client: Client,
    tx: UnboundedSender<Delta>,
    entry: KindEntry,
    kind_id: String,
    ctx: Option<String>,
    scope: Option<String>,
) {
    // Self-heal: on any watch error or stream end, rebuild the client (so
    // expired exec credentials — e.g. `aws eks get-token` — are refreshed) and
    // restart the watch with capped backoff. Stale data stays visible meanwhile.
    let mut backoff = 1u64;
    loop {
        // When the connection is namespace-scoped, watch only that namespace
        // (cluster-wide would 403). Cluster-scoped kinds (Node, …) stay all-wide.
        let api: Api<DynamicObject> = match &scope {
            Some(ns) if entry.namespaced => Api::namespaced_with(client.clone(), ns, &entry.ar),
            _ => Api::all_with(client.clone(), &entry.ar),
        };
        let mut stream = watcher::watcher(api, Config::default()).boxed();
        let mut errored = false;
        while let Some(ev) = stream.next().await {
            match ev {
                Ok(Event::Init) => {
                    let _ = tx.send(Delta::Reset);
                }
                Ok(Event::InitApply(o)) | Ok(Event::Apply(o)) => {
                    let row = normalize(
                        &entry.kind,
                        o.metadata.namespace.clone().unwrap_or_default(),
                        o.metadata.name.clone().unwrap_or_default(),
                        o.metadata.creation_timestamp.as_ref(),
                        &o.data,
                    );
                    let _ = tx.send(Delta::Applied { kind_id: kind_id.clone(), row });
                }
                Ok(Event::InitDone) => {
                    backoff = 1; // healthy again
                    let _ = tx.send(Delta::Conn(ConnState::Live));
                }
                Ok(Event::Delete(o)) => {
                    let _ = tx.send(Delta::Deleted {
                        kind_id: kind_id.clone(),
                        namespace: o.namespace().unwrap_or_default(),
                        name: o.name_any(),
                    });
                }
                Err(e) => {
                    let _ = tx.send(Delta::Conn(ConnState::Error(e.to_string())));
                    errored = true;
                    break; // rebuild client + reconnect rather than reuse stale creds
                }
            }
        }
        let _ = errored;
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(15);
        // Rebuild the client so credentials are re-fetched on reconnect.
        match rebuild_client(&ctx).await {
            Ok(c) => client = c,
            Err(e) => {
                let _ = tx.send(Delta::Conn(ConnState::Error(format!("reconnect failed: {e}"))));
            }
        }
    }
}

fn dyn_api(client: Client, entry: &KindEntry, namespace: &str) -> Api<DynamicObject> {
    if entry.namespaced {
        Api::namespaced_with(client, namespace, &entry.ar)
    } else {
        Api::all_with(client, &entry.ar)
    }
}

fn action_result(tx: &UnboundedSender<Delta>, ok: bool, message: String) {
    let _ = tx.send(Delta::ActionResult { ok, message });
}

/// Run an interactive shell in a pod container: stream stdout to the UI and
/// forward UI keystrokes to stdin. Tries bash, falls back to sh.
/// Message from the UI to an active exec session.
enum ExecMsg {
    Stdin(Vec<u8>),
    Resize(u16, u16),
}

/// Drive an interactive shell via `kubectl exec` in a PTY. kube-rs only does
/// WebSocket exec, which some API servers reject (e.g. EKS over HTTP/2 → 404);
/// kubectl falls back to SPDY and "just works" the way Lens does.
async fn exec_session(
    tx: UnboundedSender<Delta>,
    ctx: Option<String>,
    namespace: String,
    name: String,
    container: Option<String>,
    mut in_rx: UnboundedReceiver<ExecMsg>,
) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::{Read, Write};

    let pair = match native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(Delta::ExecData(format!("\r\n[pty failed] {e}\r\n")));
            let _ = tx.send(Delta::ExecEnd);
            return;
        }
    };

    let mut cmd = CommandBuilder::new("kubectl");
    // Inherit env (PATH, KUBECONFIG, AWS_*) so kubectl + auth plugins work.
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.arg("exec");
    cmd.arg("-i");
    cmd.arg("-t");
    if let Some(c) = &ctx {
        cmd.arg("--context");
        cmd.arg(c);
    }
    cmd.arg("-n");
    cmd.arg(&namespace);
    cmd.arg(&name);
    if let Some(c) = &container {
        cmd.arg("-c");
        cmd.arg(c);
    }
    cmd.arg("--");
    cmd.arg("/bin/sh");
    cmd.arg("-c");
    cmd.arg("exec bash 2>/dev/null || exec sh");

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Delta::ExecData(format!("\r\n[kubectl exec failed] {e}\r\n")));
            let _ = tx.send(Delta::ExecEnd);
            return;
        }
    };
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().ok();
    let mut writer = pair.master.take_writer().ok();
    let master = pair.master;

    // Read PTY output on a blocking thread → ExecData.
    let read_tx = tx.clone();
    if let Some(mut reader) = reader {
        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if read_tx.send(Delta::ExecData(String::from_utf8_lossy(&buf[..n]).into_owned())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // Forward stdin/resize (blocking writes off the async runtime).
    while let Some(msg) = in_rx.recv().await {
        match msg {
            ExecMsg::Stdin(bytes) => {
                if let Some(w) = writer.as_mut() {
                    if w.write_all(&bytes).is_err() || w.flush().is_err() {
                        break;
                    }
                }
            }
            ExecMsg::Resize(cols, rows) => {
                let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
        }
    }
    // Session ended (sender dropped) → kill the kubectl child.
    let _ = child.kill();
    let _ = tx.send(Delta::ExecEnd);
}

/// Poll metrics.k8s.io for pod + node usage every 10s. Silent if absent.
async fn poll_metrics(client: Client, tx: UnboundedSender<Delta>) {
    let pod_ar = ApiResource {
        group: "metrics.k8s.io".into(),
        version: "v1beta1".into(),
        api_version: "metrics.k8s.io/v1beta1".into(),
        kind: "PodMetrics".into(),
        plural: "pods".into(),
    };
    let node_ar = ApiResource {
        group: "metrics.k8s.io".into(),
        version: "v1beta1".into(),
        api_version: "metrics.k8s.io/v1beta1".into(),
        kind: "NodeMetrics".into(),
        plural: "nodes".into(),
    };
    loop {
        let mut samples: Vec<MetricSample> = Vec::new();

        let pods: Api<DynamicObject> = Api::all_with(client.clone(), &pod_ar);
        if let Ok(list) = pods.list(&Default::default()).await {
            for o in list {
                let (mut cpu, mut mem) = (0i64, 0i64);
                if let Some(cs) = o.data["containers"].as_array() {
                    for c in cs {
                        cpu += parse_cpu_milli(c["usage"]["cpu"].as_str().unwrap_or("0"));
                        mem += parse_mem_bytes(c["usage"]["memory"].as_str().unwrap_or("0"));
                    }
                }
                samples.push(MetricSample {
                    namespace: o.metadata.namespace.clone().unwrap_or_default(),
                    name: o.metadata.name.clone().unwrap_or_default(),
                    cpu_milli: cpu,
                    mem_bytes: mem,
                });
            }
        }

        let nodes: Api<DynamicObject> = Api::all_with(client.clone(), &node_ar);
        if let Ok(list) = nodes.list(&Default::default()).await {
            for o in list {
                samples.push(MetricSample {
                    namespace: String::new(),
                    name: o.metadata.name.clone().unwrap_or_default(),
                    cpu_milli: parse_cpu_milli(o.data["usage"]["cpu"].as_str().unwrap_or("0")),
                    mem_bytes: parse_mem_bytes(o.data["usage"]["memory"].as_str().unwrap_or("0")),
                });
            }
        }

        if !samples.is_empty() {
            let _ = tx.send(Delta::Metrics(samples));
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}

/// Parse a k8s CPU quantity to millicores (e.g. "123m", "1500000000n", "2").
fn parse_cpu_milli(s: &str) -> i64 {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('n') {
        n.parse::<i64>().unwrap_or(0) / 1_000_000
    } else if let Some(u) = s.strip_suffix('u') {
        u.parse::<i64>().unwrap_or(0) / 1_000
    } else if let Some(m) = s.strip_suffix('m') {
        m.parse::<i64>().unwrap_or(0)
    } else {
        (s.parse::<f64>().unwrap_or(0.0) * 1000.0) as i64
    }
}

/// Parse a k8s memory quantity to bytes (e.g. "256Mi", "1024Ki", "1Gi").
fn parse_mem_bytes(s: &str) -> i64 {
    let s = s.trim();
    let units = [
        ("Ki", 1024i64),
        ("Mi", 1024 * 1024),
        ("Gi", 1024 * 1024 * 1024),
        ("Ti", 1024i64 * 1024 * 1024 * 1024),
        ("K", 1000),
        ("M", 1_000_000),
        ("G", 1_000_000_000),
    ];
    for (suf, mult) in units {
        if let Some(n) = s.strip_suffix(suf) {
            return n.trim().parse::<i64>().unwrap_or(0) * mult;
        }
    }
    s.parse::<i64>().unwrap_or(0)
}

async fn client_for_context(name: &str) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    let kubeconfig = kube::config::Kubeconfig::read()?;
    let opts = kube::config::KubeConfigOptions {
        context: Some(name.to_string()),
        cluster: None,
        user: None,
    };
    let config = kube::Config::from_custom_kubeconfig(kubeconfig, &opts).await?;
    Ok(Client::try_from(config)?)
}

async fn fetch_manifest(
    client: Client,
    tx: UnboundedSender<Delta>,
    entry: KindEntry,
    namespace: String,
    name: String,
) {
    let api = dyn_api(client, &entry, &namespace);
    match api.get(&name).await {
        Ok(mut obj) => {
            obj.metadata.managed_fields = None;
            match serde_yaml::to_string(&obj) {
                Ok(yaml) => {
                    let _ = tx.send(Delta::Manifest { namespace, name, yaml });
                }
                Err(e) => {
                    let _ = tx.send(Delta::ManifestErr { namespace, name, error: e.to_string() });
                }
            }
        }
        Err(e) => {
            let _ = tx.send(Delta::ManifestErr { namespace, name, error: e.to_string() });
        }
    }
}

/// Stream one pod/container, tagging each line with `source`/`idx`.
async fn stream_one(
    api: Api<Pod>,
    tx: UnboundedSender<Delta>,
    pod: String,
    container: String,
    source: String,
    idx: u8,
) {
    let lp = LogParams {
        follow: true,
        tail_lines: Some(400),
        timestamps: true,
        container: Some(container),
        ..Default::default()
    };
    match api.log_stream(&pod, &lp).await {
        Ok(stream) => {
            let mut lines = stream.lines();
            while let Some(item) = lines.next().await {
                match item {
                    Ok(line) => {
                        if tx.send(Delta::LogLine { source: source.clone(), idx, line }).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        }
        Err(e) => {
            let _ = tx.send(Delta::LogLine { source, idx, line: format!("[stream failed] {e}") });
        }
    }
}

/// Stream a pod's logs. `container=None` streams every container concurrently.
/// `group_by_pod`: color/label by pod (controller view) vs by container (pod view).
async fn pod_all_logs(
    api: Api<Pod>,
    tx: UnboundedSender<Delta>,
    pod: String,
    pod_idx: u8,
    container: Option<String>,
    group_by_pod: bool,
) {
    let names: Vec<String> = match container {
        Some(c) => vec![c],
        None => api
            .get(&pod)
            .await
            .ok()
            .and_then(|p| p.spec)
            .map(|s| s.containers.into_iter().map(|c| c.name).collect())
            .unwrap_or_default(),
    };
    let multi = names.len() > 1;
    let futs = names.into_iter().enumerate().map(|(i, cn)| {
        let (api, tx, pod) = (api.clone(), tx.clone(), pod.clone());
        let (source, idx) = if group_by_pod {
            (if multi { format!("{pod}/{cn}") } else { pod.clone() }, pod_idx)
        } else {
            (cn.clone(), (i % 8) as u8)
        };
        async move { stream_one(api, tx, pod, cn, source, idx).await }
    });
    futures::future::join_all(futs).await;
}

/// Merge-tail logs across the pods selected by a controller's label selector.
async fn stream_controller_logs(
    client: Client,
    tx: UnboundedSender<Delta>,
    namespace: String,
    name: String,
    container: Option<String>,
) {
    // Resolve the controller's pod selector via the typed Deployment shape
    // (StatefulSet/DaemonSet share the same spec.selector layout).
    let dep_api: Api<Deployment> = Api::namespaced(client.clone(), &namespace);
    let selector = match dep_api.get(&name).await {
        Ok(d) => d
            .spec
            .and_then(|s| s.selector.match_labels)
            .map(|m| m.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(","))
            .unwrap_or_default(),
        Err(e) => {
            let _ = tx.send(Delta::LogLine { source: name, idx: 0, line: format!("[resolve failed] {e}") });
            return;
        }
    };
    let pod_api: Api<Pod> = Api::namespaced(client, &namespace);
    let pods = match pod_api.list(&ListParams::default().labels(&selector)).await {
        Ok(l) => l.items,
        Err(e) => {
            let _ = tx.send(Delta::LogLine { source: name, idx: 0, line: format!("[list failed] {e}") });
            return;
        }
    };
    if pods.is_empty() {
        let _ = tx.send(Delta::LogLine { source: name, idx: 0, line: "[no running pods]".into() });
        return;
    }
    let loops = pods.into_iter().enumerate().map(|(i, p)| {
        let api = pod_api.clone();
        let tx = tx.clone();
        let pod_name = p.name_any();
        let container = container.clone();
        async move { pod_all_logs(api, tx, pod_name, (i % 8) as u8, container, true).await }
    });
    futures::future::join_all(loops).await;
}

async fn owning_deployment(client: &Client, namespace: &str, pod: &Pod) -> Option<String> {
    let rs_name = pod
        .owner_references()
        .iter()
        .find(|o| o.kind == "ReplicaSet")
        .map(|o| o.name.clone())?;
    let rs: ReplicaSet = Api::namespaced(client.clone(), namespace).get(&rs_name).await.ok()?;
    rs.owner_references()
        .iter()
        .find(|o| o.kind == "Deployment")
        .map(|o| o.name.clone())
}

async fn delete(
    client: Client,
    tx: UnboundedSender<Delta>,
    entry: KindEntry,
    namespace: String,
    name: String,
    force: bool,
) {
    let api = dyn_api(client, &entry, &namespace);
    // Force delete skips graceful termination (grace period 0).
    let dp = if force {
        DeleteParams { grace_period_seconds: Some(0), ..Default::default() }
    } else {
        DeleteParams::default()
    };
    let verb = if force { "Force-deleting" } else { "Deleting" };
    match api.delete(&name, &dp).await {
        Ok(_) => action_result(&tx, true, format!("{verb} {} {name}", entry.kind)),
        Err(e) => action_result(&tx, false, format!("Delete failed: {e}")),
    }
}

async fn restart(client: Client, tx: UnboundedSender<Delta>, kind: String, namespace: String, name: String) {
    let now = k8s_openapi::jiff::Timestamp::now().to_string();
    let patch = serde_json::json!({
        "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": now
        }}}}
    });
    match kind.as_str() {
        "Deployment" => patch_deployment(&client, &tx, &namespace, &name, &patch, "Rollout restart").await,
        "Pod" => {
            let pod_api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
            match pod_api.get(&name).await {
                Ok(pod) => match owning_deployment(&client, &namespace, &pod).await {
                    Some(dep) => patch_deployment(&client, &tx, &namespace, &dep, &patch, "Rollout restart").await,
                    None => match pod_api.delete(&name, &DeleteParams::default()).await {
                        Ok(_) => action_result(&tx, true, format!("Restarting pod {name}")),
                        Err(e) => action_result(&tx, false, format!("Restart failed: {e}")),
                    },
                },
                Err(e) => action_result(&tx, false, format!("Restart failed: {e}")),
            }
        }
        _ => action_result(&tx, false, format!("Restart not supported for {kind}")),
    }
}

async fn patch_deployment(
    client: &Client,
    tx: &UnboundedSender<Delta>,
    namespace: &str,
    name: &str,
    patch: &serde_json::Value,
    label: &str,
) {
    let api: Api<Deployment> = Api::namespaced(client.clone(), namespace);
    match api.patch(name, &PatchParams::default(), &Patch::Merge(patch)).await {
        Ok(_) => action_result(tx, true, format!("{label} of {name}")),
        Err(e) => action_result(tx, false, format!("{label} failed: {e}")),
    }
}

async fn scale(client: Client, tx: UnboundedSender<Delta>, kind: String, namespace: String, name: String, replicas: i32) {
    let patch = serde_json::json!({ "spec": { "replicas": replicas } });
    let target = match kind.as_str() {
        "Deployment" | "StatefulSet" | "ReplicaSet" => Some(name.clone()),
        "Pod" => {
            let pod_api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
            match pod_api.get(&name).await {
                Ok(pod) => owning_deployment(&client, &namespace, &pod).await,
                Err(e) => return action_result(&tx, false, format!("Scale failed: {e}")),
            }
        }
        _ => return action_result(&tx, false, format!("Scale not supported for {kind}")),
    };
    match target {
        Some(dep) => {
            let api: Api<Deployment> = Api::namespaced(client, &namespace);
            match api.patch(&dep, &PatchParams::default(), &Patch::Merge(&patch)).await {
                Ok(_) => action_result(&tx, true, format!("Scaled {dep} to {replicas} replicas")),
                Err(e) => action_result(&tx, false, format!("Scale failed: {e}")),
            }
        }
        None => action_result(&tx, false, "No scalable workload found".into()),
    }
}

async fn apply_yaml(
    client: Client,
    tx: UnboundedSender<Delta>,
    entry: KindEntry,
    namespace: String,
    name: String,
    yaml: String,
) {
    let value: serde_json::Value = match serde_yaml::from_str(&yaml) {
        Ok(v) => v,
        Err(e) => return action_result(&tx, false, format!("Invalid YAML: {e}")),
    };
    let api = dyn_api(client, &entry, &namespace);
    let params = PatchParams::apply("kompass").force();
    match api.patch(&name, &params, &Patch::Apply(&value)).await {
        Ok(_) => action_result(&tx, true, format!("Applied changes to {name}")),
        Err(e) => action_result(&tx, false, format!("Apply failed: {e}")),
    }
}
