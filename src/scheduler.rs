use serde::Serialize;

use crate::{ExecutionModel, ExecutionPlan};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DummyExecutionReport {
    pub graph_id: String,
    pub graph_revision: String,
    pub frame_count: usize,
    pub frames: Vec<DummyFrameReport>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DummyFrameReport {
    pub index: usize,
    pub executed_nodes: Vec<DummyNodeExecution>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DummyNodeExecution {
    pub node_id: String,
    pub kind: String,
    pub kind_version: String,
    pub execution_model: ExecutionModel,
    pub order: usize,
    pub status: &'static str,
}

pub fn run_dummy_execution(plan: &ExecutionPlan, frame_count: usize) -> DummyExecutionReport {
    let frame_count = frame_count.max(1);
    let frames = (0..frame_count)
        .map(|index| DummyFrameReport {
            index,
            executed_nodes: plan
                .nodes
                .iter()
                .map(|node| DummyNodeExecution {
                    node_id: node.node_id.clone(),
                    kind: node.kind.clone(),
                    kind_version: node.kind_version.clone(),
                    execution_model: node.execution_model.clone(),
                    order: node.order,
                    status: "simulated",
                })
                .collect(),
        })
        .collect();

    DummyExecutionReport {
        graph_id: plan.graph_id.clone(),
        graph_revision: plan.graph_revision.clone(),
        frame_count,
        frames,
    }
}

pub fn format_dummy_execution_text(report: &DummyExecutionReport) -> String {
    let mut output = format!(
        "dummy execution: {} revision {} frames={}\n",
        report.graph_id, report.graph_revision, report.frame_count
    );

    for frame in &report.frames {
        output.push_str(&format!("\nframe {}:\n", frame.index));
        for node in &frame.executed_nodes {
            output.push_str(&format!(
                "  {} {}@{} order={} status={}\n",
                node.node_id, node.kind, node.kind_version, node.order, node.status
            ));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use crate::{
        ExecutionGroup, ExecutionModel, ExecutionPlan, PlanEdge, PlanNode, run_dummy_execution,
    };

    #[test]
    fn dummy_execution_follows_plan_order_for_each_frame() {
        let plan = ExecutionPlan {
            graph_id: "graph".to_owned(),
            graph_revision: "1".to_owned(),
            nodes: vec![
                PlanNode {
                    node_id: "a".to_owned(),
                    kind: "core.value-f32".to_owned(),
                    kind_version: "0.1.0".to_owned(),
                    execution_model: ExecutionModel::Value,
                    order: 0,
                },
                PlanNode {
                    node_id: "b".to_owned(),
                    kind: "core.target".to_owned(),
                    kind_version: "0.1.0".to_owned(),
                    execution_model: ExecutionModel::Value,
                    order: 1,
                },
            ],
            edges: vec![PlanEdge {
                from_node: "a".to_owned(),
                from_port: "out".to_owned(),
                to_node: "b".to_owned(),
                to_port: "value".to_owned(),
            }],
            groups: vec![ExecutionGroup {
                execution_model: ExecutionModel::Value,
                node_ids: vec!["a".to_owned(), "b".to_owned()],
            }],
        };

        let report = run_dummy_execution(&plan, 2);
        assert_eq!(report.frame_count, 2);
        assert_eq!(report.frames[0].executed_nodes[0].node_id, "a");
        assert_eq!(report.frames[1].executed_nodes[1].node_id, "b");
    }
}
