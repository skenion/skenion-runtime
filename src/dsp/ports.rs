use super::AUDIO_SIGNAL_KIND;
use crate::{DataFlow, Edge, GraphDocument, Port, PortDirection};

pub(super) fn is_audio_signal_edge(edge: &Edge, graph: &GraphDocument) -> bool {
    let Some(from) = find_port(graph, &edge.from.node, &edge.from.port) else {
        return false;
    };
    let Some(to) = find_port(graph, &edge.to.node, &edge.to.port) else {
        return false;
    };
    is_audio_signal_output(from) && is_audio_signal_input(to)
}

fn find_port<'a>(graph: &'a GraphDocument, node_id: &str, port_id: &str) -> Option<&'a Port> {
    graph
        .nodes
        .iter()
        .find(|node| node.id == node_id)
        .and_then(|node| node.ports.iter().find(|port| port.id == port_id))
}

pub(super) fn is_audio_signal_port(port: &Port) -> bool {
    port.data_type.flow == DataFlow::Signal && port.data_type.data_kind == AUDIO_SIGNAL_KIND
}

pub(super) fn is_audio_signal_input(port: &Port) -> bool {
    port.direction == PortDirection::Input && is_audio_signal_port(port)
}

pub(super) fn is_audio_signal_output(port: &Port) -> bool {
    port.direction == PortDirection::Output && is_audio_signal_port(port)
}
