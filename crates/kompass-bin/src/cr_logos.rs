//! Vendored project logos for custom-resource icons (full-color SVGs),
//! embedded at build time and rendered via isolated data-URI <img>.

/// Raw SVG markup for a project icon key (see kompass_core::cr_icon_key).
pub fn cr_logo_svg(key: &str) -> Option<&'static str> {
    Some(match key {
        "prometheus" => include_str!("../assets/icon/cr/prometheus.svg"),
        "argo" => include_str!("../assets/icon/cr/argo.svg"),
        "cert-manager" => include_str!("../assets/icon/cr/cert-manager.svg"),
        "kyverno" => include_str!("../assets/icon/cr/kyverno.svg"),
        "external-secrets" => include_str!("../assets/icon/cr/external-secrets.svg"),
        "envoy" => include_str!("../assets/icon/cr/envoy.svg"),
        "opentelemetry" => include_str!("../assets/icon/cr/opentelemetry.svg"),
        "kubernetes" => include_str!("../assets/icon/cr/kubernetes.svg"),
        "karpenter" => include_str!("../assets/icon/cr/karpenter.svg"),
        "strimzi" => include_str!("../assets/icon/cr/strimzi.svg"),
        "elastic" => include_str!("../assets/icon/cr/elastic.svg"),
        "aws" => include_str!("../assets/icon/cr/aws.svg"),
        "cassandra" => include_str!("../assets/icon/cr/cassandra.svg"),
        "k6" => include_str!("../assets/icon/cr/k6.svg"),
        "external-dns" => include_str!("../assets/icon/cr/external-dns.svg"),
        "gateway-api" => include_str!("../assets/icon/cr/gateway-api.svg"),
        _ => return None,
    })
}
