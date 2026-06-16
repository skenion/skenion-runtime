use serde_json::Value;
use thiserror::Error;

use crate::render::PreviewDocument;

pub const RENDER_CLEAR_COLOR_KIND: &str = "render.clear-color";
pub const RENDER_FULLSCREEN_SHADER_KIND: &str = "render.fullscreen-shader";
pub const DEFAULT_CLEAR_COLOR: [f64; 4] = [0.02, 0.02, 0.025, 1.0];

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
}

pub fn render_scene_from_preview_document(
    document: &PreviewDocument,
) -> Result<RenderScene, RenderSceneBuildError> {
    if let Some(node) = document
        .graph
        .nodes
        .iter()
        .find(|node| node.kind == RENDER_FULLSCREEN_SHADER_KIND)
    {
        return fullscreen_shader_scene_from_node(node);
    }

    Ok(clear_color_scene_from_preview_document(document))
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

fn clear_color_scene_from_preview_document(document: &PreviewDocument) -> RenderScene {
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

    RenderScene::ClearColor(ClearColorScene {
        clear_color: color.map(|component| component.clamp(0.0, 1.0)),
        source_node_id: Some(node.id.clone()),
    })
}

fn fullscreen_shader_scene_from_node(
    node: &crate::GraphNode,
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
        fallback_clear_color: DEFAULT_CLEAR_COLOR,
    }))
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

    fn document_with_nodes(nodes: Vec<GraphNode>) -> PreviewDocument {
        PreviewDocument {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph: GraphDocument {
                schema: "skenion.graph".to_owned(),
                schema_version: "0.1.0".to_owned(),
                id: "render-graph".to_owned(),
                revision: "1".to_owned(),
                nodes,
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

    fn clear_node(color: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("color".to_owned(), color);
        GraphNode {
            id: "clear_1".to_owned(),
            kind: RENDER_CLEAR_COLOR_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: Vec::new(),
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
            ports: Vec::new(),
        }
    }

    fn shader_source() -> &'static str {
        r#"struct VertexOut {
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
  return vec4<f32>(0.2, 0.3, 0.8, 1.0);
}"#
    }
}
