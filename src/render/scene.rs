use serde_json::Value;

use crate::render::PreviewDocument;

pub const RENDER_CLEAR_COLOR_KIND: &str = "render.clear-color";
pub const DEFAULT_CLEAR_COLOR: [f64; 4] = [0.02, 0.02, 0.025, 1.0];

#[derive(Debug, Clone, PartialEq)]
pub struct RenderScene {
    pub clear_color: [f64; 4],
    pub source_node_id: Option<String>,
}

pub fn render_scene_from_preview_document(document: &PreviewDocument) -> RenderScene {
    let Some(node) = document
        .graph
        .nodes
        .iter()
        .find(|node| node.kind == RENDER_CLEAR_COLOR_KIND)
    else {
        return RenderScene::default();
    };

    let Some(color) = node.params.get("color").and_then(read_color) else {
        return RenderScene::default();
    };

    RenderScene {
        clear_color: color.map(|component| component.clamp(0.0, 1.0)),
        source_node_id: Some(node.id.clone()),
    }
}

impl Default for RenderScene {
    fn default() -> Self {
        Self {
            clear_color: DEFAULT_CLEAR_COLOR,
            source_node_id: None,
        }
    }
}

fn read_color(value: &Value) -> Option<[f64; 4]> {
    let values = value.as_array()?;
    let [r, g, b, a] = values.as_slice() else {
        return None;
    };
    Some([r.as_f64()?, g.as_f64()?, b.as_f64()?, a.as_f64()?])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        ExecutionPlan, GraphDocument, GraphNode,
        render::{PREVIEW_DOCUMENT_SCHEMA, PREVIEW_DOCUMENT_SCHEMA_VERSION},
    };

    #[test]
    fn extracts_valid_clear_color() {
        let document = document_with_color(json!([0.05, 0.08, 0.12, 1.0]));

        let scene = render_scene_from_preview_document(&document);

        assert_eq!(scene.clear_color, [0.05, 0.08, 0.12, 1.0]);
        assert_eq!(scene.source_node_id.as_deref(), Some("clear_1"));
    }

    #[test]
    fn clamps_out_of_range_clear_color() {
        let document = document_with_color(json!([-1.0, 0.5, 1.4, 2.0]));

        let scene = render_scene_from_preview_document(&document);

        assert_eq!(scene.clear_color, [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(scene.source_node_id.as_deref(), Some("clear_1"));
    }

    #[test]
    fn defaults_when_render_node_is_missing() {
        let mut document = document_with_color(json!([0.1, 0.2, 0.3, 1.0]));
        document.graph.nodes.clear();

        let scene = render_scene_from_preview_document(&document);

        assert_eq!(scene, RenderScene::default());
    }

    #[test]
    fn defaults_when_color_is_invalid() {
        for value in [
            json!("red"),
            json!([0.1, 0.2, 0.3]),
            json!([0.1, 0.2, 0.3, 1.0, 0.5]),
            json!([0.1, false, 0.3, 1.0]),
        ] {
            let document = document_with_color(value);

            let scene = render_scene_from_preview_document(&document);

            assert_eq!(scene, RenderScene::default());
        }
    }

    fn document_with_color(color: Value) -> PreviewDocument {
        let mut params = serde_json::Map::new();
        params.insert("color".to_owned(), color);
        PreviewDocument {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph: GraphDocument {
                schema: "skenion.graph".to_owned(),
                schema_version: "0.1.0".to_owned(),
                id: "render-graph".to_owned(),
                revision: "1".to_owned(),
                nodes: vec![GraphNode {
                    id: "clear_1".to_owned(),
                    kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
                    kind_version: "0.1.0".to_owned(),
                    params,
                    ports: Vec::new(),
                }],
                edges: Vec::new(),
            },
            plan: ExecutionPlan {
                graph_id: "render-graph".to_owned(),
                graph_revision: "1".to_owned(),
                nodes: Vec::new(),
                edges: Vec::new(),
                groups: Vec::new(),
            },
            session_revision: 1,
        }
    }
}
