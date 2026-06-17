use serde_json::Value;
use thiserror::Error;

use crate::render::PreviewDocument;
use crate::{GraphNode, PortDirection};

pub const RENDER_CLEAR_COLOR_KIND: &str = "render.clear-color";
pub const RENDER_FULLSCREEN_SHADER_KIND: &str = "render.fullscreen-shader";
pub const RENDER_OUTPUT_KIND: &str = "render.output";
pub const DEFAULT_CLEAR_COLOR: [f64; 4] = [0.02, 0.02, 0.025, 1.0];
pub const DEFAULT_SHADER_COLOR: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

#[derive(Debug, Clone, PartialEq)]
pub enum RenderScene {
    ClearColor(ClearColorScene),
    FullscreenShader(FullscreenShaderScene),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClearColorScene {
    pub clear_color: [f64; 4],
    pub source_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FullscreenShaderScene {
    pub language: ShaderLanguage,
    pub source: String,
    pub source_node_id: String,
    pub u_value: f32,
    pub u_value2: f32,
    pub u_color: [f32; 4],
    pub fallback_clear_color: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShaderLanguage {
    Wgsl,
}

#[derive(Debug, Clone, Error, PartialEq)]
pub enum RenderSceneBuildError {
    #[error("fullscreen shader node {node_id} is missing params.language")]
    MissingShaderLanguage { node_id: String },
    #[error("fullscreen shader node {node_id} uses unsupported language {language}")]
    UnsupportedShaderLanguage { node_id: String, language: String },
    #[error("fullscreen shader node {node_id} is missing non-empty params.source")]
    MissingShaderSource { node_id: String },
    #[error("fullscreen shader node {node_id} source is missing {entrypoint} entry point")]
    MissingShaderEntrypoint {
        node_id: String,
        entrypoint: &'static str,
    },
    #[error("render output node {node_id} has no incoming edge to port in")]
    RenderOutputWithoutInput { node_id: String },
    #[error("render output node {output_node_id} references missing source node {source_node_id}")]
    MissingRenderOutputSourceNode {
        output_node_id: String,
        source_node_id: String,
    },
    #[error(
        "render output node {output_node_id} references missing output port {port_id} on source node {source_node_id}"
    )]
    MissingRenderOutputSourcePort {
        output_node_id: String,
        source_node_id: String,
        port_id: String,
    },
    #[error(
        "render output node {output_node_id} is connected to unsupported render source {source_node_id} ({source_kind})"
    )]
    UnsupportedRenderOutputSource {
        output_node_id: String,
        source_node_id: String,
        source_kind: String,
    },
}

pub fn render_scene_from_preview_document(
    document: &PreviewDocument,
) -> Result<RenderScene, RenderSceneBuildError> {
    if let Some(scene) = explicit_render_output_scene(document)? {
        return Ok(scene);
    }

    legacy_render_scene(document)
}

impl RenderScene {
    pub fn renderer_label(&self) -> &'static str {
        match self {
            Self::ClearColor(_) => "clear-color",
            Self::FullscreenShader(_) => "fullscreen-shader",
        }
    }

    pub fn source_node_id(&self) -> Option<String> {
        match self {
            Self::ClearColor(scene) => scene.source_node_id.clone(),
            Self::FullscreenShader(scene) => Some(scene.source_node_id.clone()),
        }
    }

    pub fn fallback_clear_color(&self) -> [f64; 4] {
        match self {
            Self::ClearColor(scene) => scene.clear_color,
            Self::FullscreenShader(scene) => scene.fallback_clear_color,
        }
    }
}

impl Default for RenderScene {
    fn default() -> Self {
        Self::ClearColor(ClearColorScene {
            clear_color: DEFAULT_CLEAR_COLOR,
            source_node_id: None,
        })
    }
}

fn explicit_render_output_scene(
    document: &PreviewDocument,
) -> Result<Option<RenderScene>, RenderSceneBuildError> {
    let Some(node) = document
        .graph
        .nodes
        .iter()
        .find(|node| node.kind == RENDER_OUTPUT_KIND)
    else {
        return Ok(None);
    };

    let Some(edge) = document
        .graph
        .edges
        .iter()
        .find(|edge| edge.to.node == node.id && edge.to.port == "in")
    else {
        return Err(RenderSceneBuildError::RenderOutputWithoutInput {
            node_id: node.id.clone(),
        });
    };

    let Some(source_node) = document
        .graph
        .nodes
        .iter()
        .find(|candidate| candidate.id == edge.from.node)
    else {
        return Err(RenderSceneBuildError::MissingRenderOutputSourceNode {
            output_node_id: node.id.clone(),
            source_node_id: edge.from.node.clone(),
        });
    };

    if !source_has_output_port(source_node, &edge.from.port) {
        return Err(RenderSceneBuildError::MissingRenderOutputSourcePort {
            output_node_id: node.id.clone(),
            source_node_id: source_node.id.clone(),
            port_id: edge.from.port.clone(),
        });
    }

    match source_node.kind.as_str() {
        RENDER_CLEAR_COLOR_KIND => Ok(Some(clear_color_scene_from_node(source_node))),
        RENDER_FULLSCREEN_SHADER_KIND => {
            fullscreen_shader_scene_from_node(document, source_node).map(Some)
        }
        _ => Err(RenderSceneBuildError::UnsupportedRenderOutputSource {
            output_node_id: node.id.clone(),
            source_node_id: source_node.id.clone(),
            source_kind: source_node.kind.clone(),
        }),
    }
}

fn legacy_render_scene(document: &PreviewDocument) -> Result<RenderScene, RenderSceneBuildError> {
    if let Some(node) = document
        .graph
        .nodes
        .iter()
        .find(|node| node.kind == RENDER_FULLSCREEN_SHADER_KIND)
    {
        return fullscreen_shader_scene_from_node(document, node);
    }

    Ok(clear_color_scene_from_preview_document(document))
}

fn clear_color_scene_from_preview_document(document: &PreviewDocument) -> RenderScene {
    let Some(node) = document
        .graph
        .nodes
        .iter()
        .find(|node| node.kind == RENDER_CLEAR_COLOR_KIND)
    else {
        return RenderScene::default();
    };

    clear_color_scene_from_node(node)
}

fn clear_color_scene_from_node(node: &GraphNode) -> RenderScene {
    let Some(color) = node.params.get("color").and_then(read_color) else {
        return RenderScene::default();
    };

    RenderScene::ClearColor(ClearColorScene {
        clear_color: color.map(|component| component.clamp(0.0, 1.0)),
        source_node_id: Some(node.id.clone()),
    })
}

fn fullscreen_shader_scene_from_node(
    document: &PreviewDocument,
    node: &GraphNode,
) -> Result<RenderScene, RenderSceneBuildError> {
    let language = match node.params.get("language").and_then(Value::as_str) {
        Some("wgsl") => ShaderLanguage::Wgsl,
        Some(language) => {
            return Err(RenderSceneBuildError::UnsupportedShaderLanguage {
                node_id: node.id.clone(),
                language: language.to_owned(),
            });
        }
        None => {
            return Err(RenderSceneBuildError::MissingShaderLanguage {
                node_id: node.id.clone(),
            });
        }
    };

    let Some(source) = node.params.get("source").and_then(Value::as_str) else {
        return Err(RenderSceneBuildError::MissingShaderSource {
            node_id: node.id.clone(),
        });
    };
    let source = source.trim();
    if source.is_empty() {
        return Err(RenderSceneBuildError::MissingShaderSource {
            node_id: node.id.clone(),
        });
    }
    if !source.contains("fn vs_main") {
        return Err(RenderSceneBuildError::MissingShaderEntrypoint {
            node_id: node.id.clone(),
            entrypoint: "vs_main",
        });
    }
    if !source.contains("fn fs_main") {
        return Err(RenderSceneBuildError::MissingShaderEntrypoint {
            node_id: node.id.clone(),
            entrypoint: "fs_main",
        });
    }

    Ok(RenderScene::FullscreenShader(FullscreenShaderScene {
        language,
        source: source.to_owned(),
        source_node_id: node.id.clone(),
        u_value: fullscreen_shader_number_input(document, node, "u_value"),
        u_value2: fullscreen_shader_number_input(document, node, "u_value2"),
        u_color: fullscreen_shader_color_input(document, node, "u_color"),
        fallback_clear_color: DEFAULT_CLEAR_COLOR,
    }))
}

fn fullscreen_shader_number_input(
    document: &PreviewDocument,
    node: &GraphNode,
    port_id: &str,
) -> f32 {
    let Some(source_node) = fullscreen_shader_input_source(document, node, port_id) else {
        return 0.0;
    };

    if source_node.kind != "core.value-f32" {
        return 0.0;
    }

    source_node
        .params
        .get("value")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0) as f32
}

fn fullscreen_shader_color_input(
    document: &PreviewDocument,
    node: &GraphNode,
    port_id: &str,
) -> [f32; 4] {
    let Some(source_node) = fullscreen_shader_input_source(document, node, port_id) else {
        return DEFAULT_SHADER_COLOR;
    };

    if source_node.kind != "core.color-rgba" {
        return DEFAULT_SHADER_COLOR;
    }

    let Some(color) = source_node.params.get("value").and_then(read_color) else {
        return DEFAULT_SHADER_COLOR;
    };

    color.map(|component| component.clamp(0.0, 1.0) as f32)
}

fn fullscreen_shader_input_source<'a>(
    document: &'a PreviewDocument,
    node: &GraphNode,
    port_id: &str,
) -> Option<&'a GraphNode> {
    let edge = document
        .graph
        .edges
        .iter()
        .find(|edge| edge.to.node == node.id && edge.to.port == port_id)?;

    document
        .graph
        .nodes
        .iter()
        .find(|candidate| candidate.id == edge.from.node)
}

fn source_has_output_port(node: &GraphNode, port_id: &str) -> bool {
    node.ports
        .iter()
        .any(|port| port.id == port_id && port.direction == PortDirection::Output)
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
        Edge, ExecutionPlan, GraphDocument, GraphNode, Port, PortRef,
        render::{PREVIEW_DOCUMENT_SCHEMA, PREVIEW_DOCUMENT_SCHEMA_VERSION},
    };

    #[test]
    fn extracts_valid_clear_color() {
        let document = document_with_nodes(vec![clear_node(json!([0.05, 0.08, 0.12, 1.0]))]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            scene,
            RenderScene::ClearColor(ClearColorScene {
                clear_color: [0.05, 0.08, 0.12, 1.0],
                source_node_id: Some("clear_1".to_owned())
            })
        );
    }

    #[test]
    fn clamps_out_of_range_clear_color() {
        let document = document_with_nodes(vec![clear_node(json!([-1.0, 0.5, 1.4, 2.0]))]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            scene,
            RenderScene::ClearColor(ClearColorScene {
                clear_color: [0.0, 0.5, 1.0, 1.0],
                source_node_id: Some("clear_1".to_owned())
            })
        );
    }

    #[test]
    fn defaults_when_render_node_is_missing() {
        let document = document_with_nodes(Vec::new());

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

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
            let document = document_with_nodes(vec![clear_node(value)]);

            let scene = render_scene_from_preview_document(&document).expect("scene should build");

            assert_eq!(scene, RenderScene::default());
        }
    }

    #[test]
    fn extracts_fullscreen_shader_source() {
        let document =
            document_with_nodes(vec![shader_node(json!("wgsl"), json!(shader_source()))]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            scene,
            RenderScene::FullscreenShader(FullscreenShaderScene {
                language: ShaderLanguage::Wgsl,
                source: shader_source().to_owned(),
                source_node_id: "shader_1".to_owned(),
                u_value: 0.0,
                u_value2: 0.0,
                u_color: DEFAULT_SHADER_COLOR,
                fallback_clear_color: DEFAULT_CLEAR_COLOR
            })
        );
        assert_eq!(scene.renderer_label(), "fullscreen-shader");
        assert_eq!(scene.source_node_id().as_deref(), Some("shader_1"));
        assert_eq!(scene.fallback_clear_color(), DEFAULT_CLEAR_COLOR);
        assert_eq!(
            RenderScene::ClearColor(ClearColorScene {
                clear_color: [0.1, 0.2, 0.3, 1.0],
                source_node_id: Some("clear_1".to_owned())
            })
            .fallback_clear_color(),
            [0.1, 0.2, 0.3, 1.0]
        );
    }

    #[test]
    fn rejects_unsupported_shader_language() {
        let document =
            document_with_nodes(vec![shader_node(json!("glsl"), json!(shader_source()))]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::UnsupportedShaderLanguage {
                node_id: "shader_1".to_owned(),
                language: "glsl".to_owned()
            }
        );
    }

    #[test]
    fn rejects_missing_shader_language() {
        let mut node = shader_node(json!("wgsl"), json!(shader_source()));
        node.params.remove("language");
        let document = document_with_nodes(vec![node]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::MissingShaderLanguage {
                node_id: "shader_1".to_owned()
            }
        );
    }

    #[test]
    fn rejects_missing_shader_source() {
        for source in [json!(null), json!("   ")] {
            let document = document_with_nodes(vec![shader_node(json!("wgsl"), source)]);

            let error =
                render_scene_from_preview_document(&document).expect_err("scene should fail");

            assert_eq!(
                error,
                RenderSceneBuildError::MissingShaderSource {
                    node_id: "shader_1".to_owned()
                }
            );
        }
    }

    #[test]
    fn rejects_missing_shader_entrypoints() {
        let document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!("@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"),
        )]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::MissingShaderEntrypoint {
                node_id: "shader_1".to_owned(),
                entrypoint: "vs_main"
            }
        );

        let document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!(
                "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }"
            ),
        )]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::MissingShaderEntrypoint {
                node_id: "shader_1".to_owned(),
                entrypoint: "fs_main"
            }
        );
    }

    #[test]
    fn prefers_fullscreen_shader_over_clear_color() {
        let document = document_with_nodes(vec![
            clear_node(json!([0.05, 0.08, 0.12, 1.0])),
            shader_node(json!("wgsl"), json!(shader_source())),
        ]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert!(matches!(scene, RenderScene::FullscreenShader(_)));
    }

    #[test]
    fn selects_clear_color_connected_to_render_output() {
        let document = document_with_edges(
            vec![
                clear_node(json!([0.05, 0.08, 0.12, 1.0])),
                shader_node(json!("wgsl"), json!(shader_source())),
                output_node("output_1"),
            ],
            vec![edge("clear_1", "out", "output_1", "in")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            scene,
            RenderScene::ClearColor(ClearColorScene {
                clear_color: [0.05, 0.08, 0.12, 1.0],
                source_node_id: Some("clear_1".to_owned())
            })
        );
    }

    #[test]
    fn selects_fullscreen_shader_connected_to_render_output() {
        let document = document_with_edges(
            vec![
                clear_node(json!([0.05, 0.08, 0.12, 1.0])),
                shader_node(json!("wgsl"), json!(shader_source())),
                output_node("output_1"),
            ],
            vec![edge("shader_1", "out", "output_1", "in")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert!(matches!(scene, RenderScene::FullscreenShader(_)));
        assert_eq!(scene.source_node_id().as_deref(), Some("shader_1"));
    }

    #[test]
    fn fullscreen_shader_defaults_u_value_to_zero() {
        let document =
            document_with_nodes(vec![shader_node(json!("wgsl"), json!(shader_source()))]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
        assert_eq!(shader_u_value2(&scene), 0.0);
        assert_eq!(shader_u_color(&scene), DEFAULT_SHADER_COLOR);
    }

    #[test]
    fn fullscreen_shader_reads_connected_value_node() {
        let document = document_with_edges(
            vec![
                value_node_with_value(json!(0.42)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "u_value")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.42);
    }

    #[test]
    fn fullscreen_shader_reads_connected_second_value_node() {
        let document = document_with_edges(
            vec![
                value_node_with_id_and_value("value_2", json!(0.73)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_2", "value", "shader_1", "u_value2")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value2(&scene), 0.73);
    }

    #[test]
    fn fullscreen_shader_reads_connected_color_node() {
        let document = document_with_edges(
            vec![
                color_node_with_value(json!([1.2, 0.5, -0.25, 0.8])),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("color_1", "value", "shader_1", "u_color")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_color(&scene), [1.0, 0.5, 0.0, 0.8]);
    }

    #[test]
    fn fullscreen_shader_clamps_connected_value_node() {
        for (value, expected) in [(json!(-0.25), 0.0), (json!(1.25), 1.0)] {
            let document = document_with_edges(
                vec![
                    value_node_with_value(value),
                    shader_node(json!("wgsl"), json!(shader_source())),
                ],
                vec![edge("value_1", "value", "shader_1", "u_value")],
            );

            let scene = render_scene_from_preview_document(&document).expect("scene should build");

            assert_eq!(shader_u_value(&scene), expected);
        }
    }

    #[test]
    fn fullscreen_shader_ignores_incompatible_u_value_source() {
        let document = document_with_edges(
            vec![
                clear_node(json!([0.1, 0.2, 0.3, 1.0])),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("clear_1", "out", "shader_1", "u_value")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
    }

    #[test]
    fn fullscreen_shader_ignores_incompatible_u_color_source() {
        let document = document_with_edges(
            vec![
                value_node_with_value(json!(0.42)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "u_color")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_color(&scene), DEFAULT_SHADER_COLOR);
    }

    #[test]
    fn fullscreen_shader_defaults_u_value_for_missing_source_node() {
        let document = document_with_edges(
            vec![shader_node(json!("wgsl"), json!(shader_source()))],
            vec![edge("missing_value", "value", "shader_1", "u_value")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
    }

    #[test]
    fn fullscreen_shader_defaults_u_color_for_missing_source_node() {
        let document = document_with_edges(
            vec![shader_node(json!("wgsl"), json!(shader_source()))],
            vec![edge("missing_color", "value", "shader_1", "u_color")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_color(&scene), DEFAULT_SHADER_COLOR);
    }

    #[test]
    fn fullscreen_shader_defaults_u_value_for_non_numeric_value() {
        let document = document_with_edges(
            vec![
                value_node_with_value(json!("not-a-number")),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "u_value")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
    }

    #[test]
    fn fullscreen_shader_defaults_u_color_for_invalid_value() {
        for value in [
            json!("not-a-color"),
            json!([1.0, 0.5, 0.25]),
            json!([1.0, false, 0.25, 1.0]),
        ] {
            let document = document_with_edges(
                vec![
                    color_node_with_value(value),
                    shader_node(json!("wgsl"), json!(shader_source())),
                ],
                vec![edge("color_1", "value", "shader_1", "u_color")],
            );

            let scene = render_scene_from_preview_document(&document).expect("scene should build");

            assert_eq!(shader_u_color(&scene), DEFAULT_SHADER_COLOR);
        }
    }

    #[test]
    fn fullscreen_shader_defaults_u_value_for_missing_value_param() {
        let document = document_with_edges(
            vec![
                value_node(),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "u_value")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
    }

    #[test]
    #[should_panic(expected = "expected fullscreen shader scene")]
    fn shader_u_value_helper_rejects_non_shader_scene() {
        let scene = RenderScene::ClearColor(ClearColorScene {
            clear_color: DEFAULT_CLEAR_COLOR,
            source_node_id: None,
        });

        let _ = shader_u_value(&scene);
    }

    #[test]
    #[should_panic(expected = "expected fullscreen shader scene")]
    fn shader_u_value2_helper_rejects_non_shader_scene() {
        let scene = RenderScene::ClearColor(ClearColorScene {
            clear_color: DEFAULT_CLEAR_COLOR,
            source_node_id: None,
        });

        let _ = shader_u_value2(&scene);
    }

    #[test]
    #[should_panic(expected = "expected fullscreen shader scene")]
    fn shader_u_color_helper_rejects_non_shader_scene() {
        let scene = RenderScene::ClearColor(ClearColorScene {
            clear_color: DEFAULT_CLEAR_COLOR,
            source_node_id: None,
        });

        let _ = shader_u_color(&scene);
    }

    #[test]
    fn rejects_render_output_without_input_edge() {
        let document = document_with_nodes(vec![
            clear_node(json!([0.05, 0.08, 0.12, 1.0])),
            output_node("output_1"),
        ]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::RenderOutputWithoutInput {
                node_id: "output_1".to_owned()
            }
        );
    }

    #[test]
    fn rejects_render_output_source_without_matching_output_port() {
        let mut clear = clear_node(json!([0.05, 0.08, 0.12, 1.0]));
        clear.ports.clear();
        let document = document_with_edges(
            vec![clear, output_node("output_1")],
            vec![edge("clear_1", "out", "output_1", "in")],
        );

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::MissingRenderOutputSourcePort {
                output_node_id: "output_1".to_owned(),
                source_node_id: "clear_1".to_owned(),
                port_id: "out".to_owned()
            }
        );
    }

    #[test]
    fn rejects_render_output_missing_source_node() {
        let document = document_with_edges(
            vec![output_node("output_1")],
            vec![edge("missing", "out", "output_1", "in")],
        );

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::MissingRenderOutputSourceNode {
                output_node_id: "output_1".to_owned(),
                source_node_id: "missing".to_owned()
            }
        );
    }

    #[test]
    fn rejects_unsupported_render_output_source() {
        let document = document_with_edges(
            vec![value_node(), output_node("output_1")],
            vec![edge("value_1", "value", "output_1", "in")],
        );

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::UnsupportedRenderOutputSource {
                output_node_id: "output_1".to_owned(),
                source_node_id: "value_1".to_owned(),
                source_kind: "core.value-f32".to_owned()
            }
        );
    }

    #[test]
    fn selects_first_render_output_deterministically() {
        let document = document_with_edges(
            vec![
                clear_node(json!([0.1, 0.2, 0.3, 1.0])),
                shader_node(json!("wgsl"), json!(shader_source())),
                output_node("output_a"),
                output_node("output_b"),
            ],
            vec![
                edge("clear_1", "out", "output_a", "in"),
                edge("shader_1", "out", "output_b", "in"),
            ],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            scene,
            RenderScene::ClearColor(ClearColorScene {
                clear_color: [0.1, 0.2, 0.3, 1.0],
                source_node_id: Some("clear_1".to_owned())
            })
        );
    }

    fn document_with_nodes(nodes: Vec<GraphNode>) -> PreviewDocument {
        document_with_edges(nodes, Vec::new())
    }

    fn document_with_edges(nodes: Vec<GraphNode>, edges: Vec<Edge>) -> PreviewDocument {
        PreviewDocument {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph: GraphDocument {
                schema: "skenion.graph".to_owned(),
                schema_version: "0.1.0".to_owned(),
                id: "render-graph".to_owned(),
                revision: "1".to_owned(),
                nodes,
                edges,
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

    fn output_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: RENDER_OUTPUT_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: serde_json::Map::new(),
            ports: vec![gpu_input_port()],
        }
    }

    fn clear_node(color: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("color".to_owned(), color);
        GraphNode {
            id: "clear_1".to_owned(),
            kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![gpu_output_port()],
        }
    }

    fn shader_node(language: Value, source: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("language".to_owned(), language);
        params.insert("source".to_owned(), source);
        GraphNode {
            id: "shader_1".to_owned(),
            kind: RENDER_FULLSCREEN_SHADER_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![gpu_output_port()],
        }
    }

    fn value_node() -> GraphNode {
        value_node_with_params(serde_json::Map::new())
    }

    fn value_node_with_value(value: Value) -> GraphNode {
        value_node_with_id_and_value("value_1", value)
    }

    fn value_node_with_id_and_value(id: &str, value: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("value".to_owned(), value);
        value_node_with_id_and_params(id, params)
    }

    fn value_node_with_params(params: serde_json::Map<String, Value>) -> GraphNode {
        value_node_with_id_and_params("value_1", params)
    }

    fn value_node_with_id_and_params(
        id: &str,
        params: serde_json::Map<String, Value>,
    ) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: "core.value-f32".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Value",
                    "type": {
                        "flow": "value",
                        "dataKind": "number.f32"
                    }
                }))
                .expect("valid value port"),
            ],
        }
    }

    fn color_node_with_value(value: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("value".to_owned(), value);
        GraphNode {
            id: "color_1".to_owned(),
            kind: "core.color-rgba".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Color",
                    "type": {
                        "flow": "value",
                        "dataKind": "color.rgba"
                    }
                }))
                .expect("valid color port"),
            ],
        }
    }

    fn shader_u_value(scene: &RenderScene) -> f32 {
        match scene {
            RenderScene::FullscreenShader(shader) => shader.u_value,
            _ => panic!("expected fullscreen shader scene"),
        }
    }

    fn shader_u_value2(scene: &RenderScene) -> f32 {
        match scene {
            RenderScene::FullscreenShader(shader) => shader.u_value2,
            _ => panic!("expected fullscreen shader scene"),
        }
    }

    fn shader_u_color(scene: &RenderScene) -> [f32; 4] {
        match scene {
            RenderScene::FullscreenShader(shader) => shader.u_color,
            _ => panic!("expected fullscreen shader scene"),
        }
    }

    fn edge(from_node: &str, from_port: &str, to_node: &str, to_port: &str) -> Edge {
        Edge {
            from: PortRef {
                node: from_node.to_owned(),
                port: from_port.to_owned(),
            },
            to: PortRef {
                node: to_node.to_owned(),
                port: to_port.to_owned(),
            },
        }
    }

    fn gpu_output_port() -> Port {
        serde_json::from_value(json!({
            "id": "out",
            "direction": "output",
            "label": "Out",
            "type": gpu_texture_type()
        }))
        .expect("valid gpu output port")
    }

    fn gpu_input_port() -> Port {
        serde_json::from_value(json!({
            "id": "in",
            "direction": "input",
            "label": "In",
            "type": gpu_texture_type(),
            "activation": "latched"
        }))
        .expect("valid gpu input port")
    }

    fn gpu_texture_type() -> Value {
        json!({
            "flow": "resource",
            "dataKind": "gpu.texture2d",
            "format": "rgba8unorm",
            "colorSpace": "srgb"
        })
    }

    fn shader_source() -> &'static str {
        r#"struct SkenionFrame {
  resolution: vec2<f32>,
  time: f32,
  frame: u32,
  u_value: f32,
  u_value2: f32,
  _pad0: vec2<f32>,
  u_color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> skenion: SkenionFrame;

struct VertexOut {
  @builtin(position) position: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOut {
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -3.0),
    vec2<f32>(-1.0,  1.0),
    vec2<f32>( 3.0,  1.0)
  );

  var out: VertexOut;
  out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
  return out;
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
  let mix_value = clamp(skenion.u_value, 0.0, 1.0);
  let brightness = 0.25 + 0.75 * clamp(skenion.u_value2, 0.0, 1.0);
  let animated = vec3<f32>(0.2 + mix_value * 0.8, 0.3, 1.0 - mix_value);
  return vec4<f32>(mix(animated, skenion.u_color.rgb, mix_value) * brightness, skenion.u_color.a);
}"#
    }
}
