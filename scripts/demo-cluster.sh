#!/usr/bin/env bash
#
# demo-cluster.sh — spin up a throwaway kind cluster with generic demo
# resources for capturing Kompass screenshots/video. NEVER use a real cluster
# for marketing assets (leaks infra names).
#
# Usage:
#   ./scripts/demo-cluster.sh up      # create cluster + load demo resources
#   ./scripts/demo-cluster.sh kubeconfig   # print KUBECONFIG export line
#   ./scripts/demo-cluster.sh down    # tear it all down
#
# After `up`, launch Kompass against it:
#   export KUBECONFIG="$(./scripts/demo-cluster.sh kubeconfig-path)"
#   ./scripts/bundle.sh --debug --open
#
set -euo pipefail

CLUSTER="kompass-demo"
KCFG="${TMPDIR:-/tmp}/kompass-demo.kubeconfig"

up() {
  kind create cluster --name "$CLUSTER" --kubeconfig "$KCFG"
  export KUBECONFIG="$KCFG"

  # A few namespaces so the namespace switcher looks real.
  for ns in shop payments observability; do
    kubectl create namespace "$ns" >/dev/null 2>&1 || true
  done

  # --- shop: a healthy multi-replica web app + service ---
  kubectl -n shop create deployment storefront --image=nginx:1.27 --replicas=3
  kubectl -n shop expose deployment storefront --port=80 --target-port=80
  kubectl -n shop create deployment checkout --image=nginx:1.27 --replicas=2
  kubectl -n shop create configmap storefront-config \
    --from-literal=THEME=midnight --from-literal=LOCALE=en-US
  kubectl -n shop create secret generic storefront-secret \
    --from-literal=API_KEY=demo-not-a-real-key

  # --- payments: a deployment + a CronJob + a Job (variety of kinds) ---
  kubectl -n payments create deployment ledger --image=nginx:1.27 --replicas=2
  kubectl -n payments create cronjob nightly-reconcile \
    --image=busybox --schedule="0 2 * * *" -- /bin/sh -c 'echo reconciling'
  kubectl -n payments create job seed-rates --image=busybox -- \
    /bin/sh -c 'echo done'

  # --- observability: a StatefulSet-ish workload + a deliberately failing pod ---
  kubectl -n observability create deployment collector --image=nginx:1.27 --replicas=2
  # CrashLoopBackOff so per-container status squares show red — great for the shot.
  kubectl -n observability run flaky --image=busybox -- /bin/sh -c 'exit 1' || true

  echo
  echo "Demo cluster up. Point Kompass at it:"
  echo "  export KUBECONFIG=\"$KCFG\""
  echo "Then: ./scripts/bundle.sh --debug --open"
}

down() {
  kind delete cluster --name "$CLUSTER"
  rm -f "$KCFG"
  echo "Demo cluster + kubeconfig removed."
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  kubeconfig) echo "export KUBECONFIG=\"$KCFG\"" ;;
  kubeconfig-path) echo "$KCFG" ;;
  *) echo "usage: $0 {up|down|kubeconfig|kubeconfig-path}" >&2; exit 1 ;;
esac
