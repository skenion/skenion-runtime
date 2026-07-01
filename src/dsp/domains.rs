use std::collections::{BTreeMap, HashMap};

use serde_json::json;

use super::ports::{is_audio_signal_edge, is_audio_signal_port};
use super::{
    AUDIO_CLOCK_BRIDGE_KIND, AUDIO_INPUT_KIND, AUDIO_OUTPUT_KIND, AUDIO_RESAMPLE_KIND,
    AudioDspPlanError, AudioEndpointPlanNode,
};
use crate::{
    AudioClockBridgeMethod, AudioClockBridgePlan, AudioClockDomain, AudioClockDomainAuthority,
    AudioEndpoint, AudioEndpointDirection, AudioGraphPartition, GraphDocument, GraphNode,
    RuntimeIssue, plan_audio_clock_bridge,
};

pub(super) fn audio_endpoint_plan_nodes(graph: &GraphDocument) -> Vec<AudioEndpointPlanNode> {
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == AUDIO_INPUT_KIND || node.kind == AUDIO_OUTPUT_KIND)
        .map(|node| AudioEndpointPlanNode {
            node_id: node.id.clone(),
            kind: node.kind.clone(),
            clock_domain_id: audio_clock_domain_id(node),
        })
        .collect()
}

fn audio_clock_domain_id(node: &GraphNode) -> String {
    node.params
        .get("clockDomain")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("endpoint:{}", node.id))
}

pub(super) fn audio_endpoints(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
) -> Vec<AudioEndpoint> {
    endpoint_nodes
        .iter()
        .filter_map(|endpoint| {
            let node = graph
                .nodes
                .iter()
                .find(|node| node.id == endpoint.node_id)?;
            let direction = if node.kind == AUDIO_INPUT_KIND {
                AudioEndpointDirection::Input
            } else {
                AudioEndpointDirection::Output
            };
            let channel_ports = node
                .ports
                .iter()
                .filter(|port| is_audio_signal_port(port))
                .map(|port| port.id.clone())
                .collect();
            Some(AudioEndpoint {
                id: node.id.clone(),
                node_id: node.id.clone(),
                direction,
                channel_ports,
                requested_config: None,
                resolved_config: None,
                clock_domain_id: Some(endpoint.clock_domain_id.clone()),
            })
        })
        .collect()
}

pub(super) fn audio_clock_domains(
    endpoint_nodes: &[AudioEndpointPlanNode],
    sample_rate: u32,
) -> Vec<AudioClockDomain> {
    let mut domains = BTreeMap::<String, Vec<String>>::new();
    for endpoint in endpoint_nodes {
        domains
            .entry(endpoint.clock_domain_id.clone())
            .or_default()
            .push(endpoint.node_id.clone());
    }
    domains
        .into_iter()
        .map(|(id, endpoint_ids)| AudioClockDomain {
            id: id.clone(),
            authority: if id.starts_with("endpoint:") {
                AudioClockDomainAuthority::Unavailable
            } else {
                AudioClockDomainAuthority::UserConfigured
            },
            source: "runtime.audio-domain-planner.v0".to_owned(),
            sample_rate: Some(sample_rate),
            drift_compensated: None,
            shared_with: Some(endpoint_ids),
        })
        .collect()
}

pub(super) fn audio_graph_partitions(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
    audio_node_set: &std::collections::HashSet<&str>,
) -> Vec<AudioGraphPartition> {
    let mut nodes_by_domain = BTreeMap::<String, Vec<String>>::new();
    for endpoint in endpoint_nodes {
        nodes_by_domain
            .entry(endpoint.clock_domain_id.clone())
            .or_default()
            .push(endpoint.node_id.clone());
    }
    if endpoint_nodes.is_empty() {
        return Vec::new();
    }
    let default_domain = endpoint_nodes
        .iter()
        .find(|endpoint| endpoint.kind == AUDIO_OUTPUT_KIND)
        .or_else(|| endpoint_nodes.first())
        .map(|endpoint| endpoint.clock_domain_id.clone())
        .expect("endpoint_nodes is not empty");
    for node in &graph.nodes {
        if !audio_node_set.contains(node.id.as_str()) || is_audio_endpoint_kind(&node.kind) {
            continue;
        }
        nodes_by_domain
            .entry(default_domain.clone())
            .or_default()
            .push(node.id.clone());
    }
    nodes_by_domain
        .into_iter()
        .map(|(clock_domain_id, node_ids)| {
            let endpoint_ids = endpoint_nodes
                .iter()
                .filter(|endpoint| endpoint.clock_domain_id == clock_domain_id)
                .map(|endpoint| endpoint.node_id.clone())
                .collect::<Vec<_>>();
            AudioGraphPartition {
                id: format!("partition:{clock_domain_id}"),
                clock_domain_id,
                endpoint_ids,
                node_ids,
            }
        })
        .collect()
}

pub(super) fn audio_clock_bridge_plans(
    graph: &GraphDocument,
    endpoint_nodes: &[AudioEndpointPlanNode],
) -> Result<Vec<AudioClockBridgePlan>, AudioDspPlanError> {
    let nodes_by_id = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut plans = Vec::new();

    for source in endpoint_nodes
        .iter()
        .filter(|endpoint| endpoint.kind == AUDIO_INPUT_KIND)
    {
        for output in endpoint_nodes
            .iter()
            .filter(|endpoint| endpoint.kind == AUDIO_OUTPUT_KIND)
        {
            let Some(route) =
                find_audio_route_to_output(graph, &nodes_by_id, &source.node_id, &output.node_id)
            else {
                continue;
            };
            let bridge_node_id = route.explicit_bridge_node_id.as_deref();
            let source_domain = endpoint_domain(source);
            let target_domain = endpoint_domain(output);
            let mut plan = plan_audio_clock_bridge(&source_domain, &target_domain, bridge_node_id);
            if matches!(
                route.explicit_bridge_kind.as_deref(),
                Some(AUDIO_RESAMPLE_KIND)
            ) {
                plan.method = AudioClockBridgeMethod::Resample;
            }
            if plan.method == AudioClockBridgeMethod::Invalid {
                return Err(AudioDspPlanError::ClockDomainCrossingRequiresBridge {
                    source_node_id: source.node_id.clone(),
                    target_node_id: output.node_id.clone(),
                    source_clock_domain_id: source.clock_domain_id.clone(),
                    target_clock_domain_id: output.clock_domain_id.clone(),
                    issue: Box::new(RuntimeIssue::structured_error(
                        "audio-dsp.clock-domain-crossing-requires-bridge",
                        format!(
                            "audio signal route from {} domain {} to {} domain {} requires object.core.audio.clock-bridge or object.core.audio.resample",
                            source.node_id,
                            source.clock_domain_id,
                            output.node_id,
                            output.clock_domain_id
                        ),
                        json!({
                            "sourceNodeId": source.node_id.as_str(),
                            "targetNodeId": output.node_id.as_str(),
                            "sourceClockDomainId": source.clock_domain_id.as_str(),
                            "targetClockDomainId": output.clock_domain_id.as_str(),
                        }),
                    )),
                });
            }
            plans.push(plan);
        }
    }

    Ok(plans)
}

fn endpoint_domain(endpoint: &AudioEndpointPlanNode) -> AudioClockDomain {
    AudioClockDomain {
        id: endpoint.clock_domain_id.clone(),
        authority: if endpoint.clock_domain_id.starts_with("endpoint:") {
            AudioClockDomainAuthority::Unavailable
        } else {
            AudioClockDomainAuthority::UserConfigured
        },
        source: endpoint.node_id.clone(),
        sample_rate: None,
        drift_compensated: None,
        shared_with: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AudioSignalRoute {
    pub(super) explicit_bridge_node_id: Option<String>,
    pub(super) explicit_bridge_kind: Option<String>,
}

pub(super) fn find_audio_route_to_output(
    graph: &GraphDocument,
    nodes_by_id: &HashMap<&str, &GraphNode>,
    source_node_id: &str,
    output_node_id: &str,
) -> Option<AudioSignalRoute> {
    let mut stack = vec![(source_node_id.to_owned(), None::<(String, String)>)];
    let mut visited = std::collections::HashSet::<String>::new();
    while let Some((node_id, bridge)) = stack.pop() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        if node_id == output_node_id {
            return Some(AudioSignalRoute {
                explicit_bridge_node_id: bridge.as_ref().map(|(id, _)| id.clone()),
                explicit_bridge_kind: bridge.map(|(_, kind)| kind),
            });
        }
        for edge in graph.edges.iter().filter(|edge| edge.from.node == node_id) {
            if !is_audio_signal_edge(edge, graph) {
                continue;
            }
            let next_bridge = nodes_by_id
                .get(edge.to.node.as_str())
                .filter(|node| is_audio_clock_boundary_kind(&node.kind))
                .map(|node| (node.id.clone(), node.kind.clone()))
                .or_else(|| bridge.clone());
            stack.push((edge.to.node.clone(), next_bridge));
        }
    }
    None
}

fn is_audio_endpoint_kind(kind: &str) -> bool {
    kind == AUDIO_INPUT_KIND || kind == AUDIO_OUTPUT_KIND
}

fn is_audio_clock_boundary_kind(kind: &str) -> bool {
    kind == AUDIO_CLOCK_BRIDGE_KIND || kind == AUDIO_RESAMPLE_KIND
}
