use serde_json::Value;
use thiserror::Error;

use crate::render::PreviewDocument;
use crate::{
    ControlValue, GraphNode, PortDirection, analyze_shader_interface_v01,
    convert_control_value_to_data_kind, shader_interface_to_ports_v01,
    telemetry::{ShaderIssue, ShaderIssuePhase, ShaderIssueSource},
};

pub const RENDER_CLEAR_COLOR_KIND: &str = "object.core.render.clear-color";
pub const RENDER_FULLSCREEN_SHADER_KIND: &str = "object.core.render.fullscreen-shader";
pub const RENDER_OUTPUT_KIND: &str = "object.core.render.output";
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
    pub uniforms: Vec<ShaderUniformBinding>,
    pub fallback_clear_color: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShaderUniformBinding {
    pub id: String,
    pub value: ShaderUniformValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShaderUniformValue {
    F32(f32),
    I32(i32),
    U32(u32),
    Bool(bool),
    ColorRgba([f32; 4]),
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
    #[error("fullscreen shader node {node_id} source declares reserved {entrypoint} entry point")]
    ReservedShaderEntrypoint {
        node_id: String,
        entrypoint: &'static str,
    },
    #[error("fullscreen shader node {node_id} has invalid interface: {message}")]
    InvalidShaderInterface {
        node_id: String,
        message: String,
        issues: Vec<ShaderIssue>,
    },
    #[error("fullscreen shader node {node_id} graph ports do not match shader annotations")]
    ShaderInterfacePortsOutOfSync { node_id: String },
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

pub(crate) fn render_scene_from_preview_document(
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
    if source.contains("fn vs_main") {
        return Err(RenderSceneBuildError::ReservedShaderEntrypoint {
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
    let analysis = analyze_shader_interface_v01(source);
    if !analysis.ok {
        let message = analysis
            .issues
            .iter()
            .map(|issue| issue.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(RenderSceneBuildError::InvalidShaderInterface {
            node_id: node.id.clone(),
            message,
            issues: analysis.issues.iter().map(shader_analysis_issue).collect(),
        });
    }
    let expected_ports = shader_interface_to_ports_v01(&analysis.shader_interface);
    if node.ports != expected_ports {
        return Err(RenderSceneBuildError::ShaderInterfacePortsOutOfSync {
            node_id: node.id.clone(),
        });
    }

    Ok(RenderScene::FullscreenShader(FullscreenShaderScene {
        language,
        source: source.to_owned(),
        source_node_id: node.id.clone(),
        uniforms: analysis
            .shader_interface
            .uniforms
            .iter()
            .map(|uniform| ShaderUniformBinding {
                id: uniform.id.clone(),
                value: shader_uniform_value(document, node, uniform),
            })
            .collect(),
        fallback_clear_color: DEFAULT_CLEAR_COLOR,
    }))
}

impl RenderSceneBuildError {
    pub fn shader_issues(&self) -> Vec<ShaderIssue> {
        match self {
            Self::InvalidShaderInterface { issues, .. } => issues.clone(),
            Self::ShaderInterfacePortsOutOfSync { node_id } => vec![ShaderIssue::error(
                ShaderIssuePhase::SourceSync,
                "shader-interface-ports-out-of-sync",
                format!(
                    "fullscreen shader node {node_id} graph ports do not match shader annotations"
                ),
                ShaderIssueSource::Runtime,
            )],
            Self::MissingShaderEntrypoint {
                node_id,
                entrypoint,
            } => vec![ShaderIssue::error(
                ShaderIssuePhase::WgslGeneration,
                "missing-shader-entrypoint",
                format!(
                    "fullscreen shader node {node_id} source is missing {entrypoint} entry point"
                ),
                ShaderIssueSource::User,
            )],
            Self::ReservedShaderEntrypoint {
                node_id,
                entrypoint,
            } => vec![ShaderIssue::error(
                ShaderIssuePhase::WgslGeneration,
                "reserved-shader-entrypoint",
                format!(
                    "fullscreen shader node {node_id} source declares reserved {entrypoint} entry point"
                ),
                ShaderIssueSource::User,
            )],
            Self::MissingShaderLanguage { node_id } => vec![ShaderIssue::error(
                ShaderIssuePhase::SourceSync,
                "missing-shader-language",
                format!("fullscreen shader node {node_id} is missing params.language"),
                ShaderIssueSource::User,
            )],
            Self::UnsupportedShaderLanguage { node_id, language } => vec![ShaderIssue::error(
                ShaderIssuePhase::SourceSync,
                "unsupported-shader-language",
                format!("fullscreen shader node {node_id} uses unsupported language {language}"),
                ShaderIssueSource::User,
            )],
            Self::MissingShaderSource { node_id } => vec![ShaderIssue::error(
                ShaderIssuePhase::SourceSync,
                "missing-shader-source",
                format!("fullscreen shader node {node_id} is missing non-empty params.source"),
                ShaderIssueSource::User,
            )],
            Self::RenderOutputWithoutInput { node_id } => vec![ShaderIssue::error(
                ShaderIssuePhase::RenderPipeline,
                "render-output-without-input",
                format!("render output node {node_id} has no incoming edge to port in"),
                ShaderIssueSource::Runtime,
            )],
            Self::MissingRenderOutputSourceNode {
                output_node_id,
                source_node_id,
            } => vec![ShaderIssue::error(
                ShaderIssuePhase::RenderPipeline,
                "missing-render-output-source-node",
                format!(
                    "render output node {output_node_id} references missing source node {source_node_id}"
                ),
                ShaderIssueSource::Runtime,
            )],
            Self::MissingRenderOutputSourcePort {
                output_node_id,
                source_node_id,
                port_id,
            } => vec![ShaderIssue::error(
                ShaderIssuePhase::RenderPipeline,
                "missing-render-output-source-port",
                format!(
                    "render output node {output_node_id} references missing output port {port_id} on source node {source_node_id}"
                ),
                ShaderIssueSource::Runtime,
            )],
            Self::UnsupportedRenderOutputSource {
                output_node_id,
                source_node_id,
                source_kind,
            } => vec![ShaderIssue::error(
                ShaderIssuePhase::RenderPipeline,
                "unsupported-render-output-source",
                format!(
                    "render output node {output_node_id} is connected to unsupported render source {source_node_id} ({source_kind})"
                ),
                ShaderIssueSource::Runtime,
            )],
        }
    }
}

fn shader_analysis_issue(issue: &crate::ShaderInterfaceIssue) -> ShaderIssue {
    ShaderIssue::error(
        ShaderIssuePhase::InterfaceAnalysis,
        issue.code.clone(),
        issue.message.clone(),
        ShaderIssueSource::User,
    )
    .with_line_column(issue.line, None)
    .with_uniform_id(issue.uniform_id.clone())
}

fn shader_uniform_value(
    document: &PreviewDocument,
    node: &GraphNode,
    uniform: &crate::ShaderUniform,
) -> ShaderUniformValue {
    let connected =
        resolve_control_value_at_input(document, &node.id, &uniform.id).and_then(|value| {
            convert_control_value_to_data_kind(
                &value,
                &uniform.data_type.data_kind,
                first_format(&uniform.data_type),
            )
        });
    match uniform.data_type.data_kind.as_str() {
        "value.core.float32" => connected
            .as_ref()
            .and_then(ControlValue::as_f32)
            .map_or_else(
                || ShaderUniformValue::F32(default_f32(&uniform.default)),
                ShaderUniformValue::F32,
            ),
        "value.core.int32" => connected
            .as_ref()
            .and_then(ControlValue::as_i32)
            .map_or_else(
                || ShaderUniformValue::I32(default_i32(&uniform.default)),
                ShaderUniformValue::I32,
            ),
        "value.core.uint32" => connected
            .as_ref()
            .and_then(ControlValue::as_u32)
            .map_or_else(
                || ShaderUniformValue::U32(default_u32(&uniform.default)),
                ShaderUniformValue::U32,
            ),
        "value.core.bool" => connected
            .as_ref()
            .and_then(ControlValue::as_bool)
            .map_or_else(
                || ShaderUniformValue::Bool(default_bool(&uniform.default)),
                ShaderUniformValue::Bool,
            ),
        "value.core.color" => connected
            .as_ref()
            .and_then(ControlValue::as_rgba_f32)
            .map_or_else(
                || ShaderUniformValue::ColorRgba(default_color(&uniform.default)),
                ShaderUniformValue::ColorRgba,
            ),
        _ => ShaderUniformValue::F32(0.0),
    }
}

fn first_format(data_type: &crate::DataType) -> Option<&str> {
    data_type
        .format
        .as_ref()
        .and_then(|format| format.values().into_iter().next())
}

fn default_f32(value: &Option<serde_json::Value>) -> f32 {
    value
        .as_ref()
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0) as f32
}

fn default_i32(value: &Option<serde_json::Value>) -> i32 {
    value
        .as_ref()
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0) as i32
}

fn default_u32(value: &Option<serde_json::Value>) -> u32 {
    value
        .as_ref()
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}

fn default_bool(value: &Option<serde_json::Value>) -> bool {
    value
        .as_ref()
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn default_color(value: &Option<serde_json::Value>) -> [f32; 4] {
    value
        .as_ref()
        .and_then(read_color_f32)
        .unwrap_or(DEFAULT_SHADER_COLOR)
}

fn read_color_f32(value: &Value) -> Option<[f32; 4]> {
    read_color(value).map(|color| color.map(|component| component.clamp(0.0, 1.0) as f32))
}

pub(crate) fn resolve_control_value_at_input(
    document: &PreviewDocument,
    target_node_id: &str,
    target_port_id: &str,
) -> Option<ControlValue> {
    let edge = document
        .graph
        .edges
        .iter()
        .find(|edge| edge.to.node == target_node_id && edge.to.port == target_port_id)?;

    if edge.from.port != "value" {
        return None;
    }

    let source_node = document
        .graph
        .nodes
        .iter()
        .find(|candidate| candidate.id == edge.from.node)?;

    document
        .control_state
        .output_value_for_node(source_node, &edge.from.port)
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
            json!([false, 0.2, 0.3, 1.0]),
            json!([0.1, false, 0.3, 1.0]),
            json!([0.1, 0.2, false, 1.0]),
            json!([0.1, 0.2, 0.3, false]),
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
                uniforms: vec![
                    ShaderUniformBinding {
                        id: "speed".to_owned(),
                        value: ShaderUniformValue::F32(0.0),
                    },
                    ShaderUniformBinding {
                        id: "phase".to_owned(),
                        value: ShaderUniformValue::F32(0.0),
                    },
                    ShaderUniformBinding {
                        id: "tint".to_owned(),
                        value: ShaderUniformValue::ColorRgba(DEFAULT_SHADER_COLOR),
                    },
                ],
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
    fn rejects_reserved_or_missing_shader_entrypoints() {
        let document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!(
                "@vertex fn vs_main() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0); }\n@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
            ),
        )]);

        let error = render_scene_from_preview_document(&document).expect_err("scene should fail");

        assert_eq!(
            error,
            RenderSceneBuildError::ReservedShaderEntrypoint {
                node_id: "shader_1".to_owned(),
                entrypoint: "vs_main"
            }
        );

        let document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!("fn helper() -> f32 { return 1.0; }"),
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
    fn rejects_invalid_or_unsynced_shader_interfaces() {
        let invalid = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!(
                "// @skenion.uniform bad vec3\n@fragment\nfn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }"
            ),
        )]);
        let error = render_scene_from_preview_document(&invalid).expect_err("scene should fail");
        assert!(matches!(
            error,
            RenderSceneBuildError::InvalidShaderInterface { node_id, message, issues }
                if node_id == "shader_1"
                    && message.contains("unsupported uniform type")
                    && issues[0].phase == ShaderIssuePhase::InterfaceAnalysis
                    && issues[0].line == Some(1)
        ));

        let mut node = shader_node(json!("wgsl"), json!(shader_source()));
        node.ports = vec![gpu_output_port()];
        let unsynced = document_with_nodes(vec![node]);
        let error = render_scene_from_preview_document(&unsynced).expect_err("scene should fail");
        assert_eq!(
            error,
            RenderSceneBuildError::ShaderInterfacePortsOutOfSync {
                node_id: "shader_1".to_owned()
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
    fn fullscreen_shader_reads_i32_and_bool_uniform_defaults() {
        let document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!(typed_shader_source()),
        )]);

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            shader_uniform(&scene, "iterations"),
            &ShaderUniformValue::I32(8)
        );
        assert_eq!(
            shader_uniform(&scene, "enabled"),
            &ShaderUniformValue::Bool(true)
        );
    }

    #[test]
    fn fullscreen_shader_reads_connected_i32_and_bool_uniforms() {
        let mut document = document_with_edges(
            vec![
                i32_node_with_value("iterations_1", 12),
                bool_payload_source_node("enabled_1"),
                shader_node(json!("wgsl"), json!(typed_shader_source())),
            ],
            vec![
                edge("iterations_1", "value", "shader_1", "iterations"),
                edge("enabled_1", "value", "shader_1", "enabled"),
            ],
        );
        document
            .control_state
            .values
            .insert("enabled_1".to_owned(), ControlValue::bool(false));

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            shader_uniform(&scene, "iterations"),
            &ShaderUniformValue::I32(12)
        );
        assert_eq!(
            shader_uniform(&scene, "enabled"),
            &ShaderUniformValue::Bool(false)
        );
    }

    #[test]
    fn fullscreen_shader_reads_uint_uniform_defaults_and_connections() {
        let default_document = document_with_nodes(vec![shader_node(
            json!("wgsl"),
            json!(uint_shader_source()),
        )]);
        let default_scene =
            render_scene_from_preview_document(&default_document).expect("scene should build");
        assert_eq!(
            shader_uniform(&default_scene, "count"),
            &ShaderUniformValue::U32(4)
        );

        let connected_document = document_with_edges(
            vec![
                u32_node_with_value("count_1", u64::from(u32::MAX) + 10),
                shader_node(json!("wgsl"), json!(uint_shader_source())),
            ],
            vec![edge("count_1", "value", "shader_1", "count")],
        );
        let connected_scene =
            render_scene_from_preview_document(&connected_document).expect("scene should build");

        assert_eq!(
            shader_uniform(&connected_scene, "count"),
            &ShaderUniformValue::U32(u32::MAX)
        );
    }

    #[test]
    fn fullscreen_shader_converts_numeric_uniform_inputs_and_defaults_incompatible_bool() {
        let document = document_with_edges(
            vec![
                value_node_with_id_and_value("wrong_iterations", json!(4.0)),
                value_node_with_id_and_value("wrong_enabled", json!(1.0)),
                shader_node(json!("wgsl"), json!(typed_shader_source())),
            ],
            vec![
                edge("wrong_iterations", "value", "shader_1", "iterations"),
                edge("wrong_enabled", "value", "shader_1", "enabled"),
            ],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            shader_uniform(&scene, "iterations"),
            &ShaderUniformValue::I32(4)
        );
        assert_eq!(
            shader_uniform(&scene, "enabled"),
            &ShaderUniformValue::Bool(true)
        );
    }

    #[test]
    fn fullscreen_shader_converts_int_and_uint_sources_to_float_uniforms() {
        let int_document = document_with_edges(
            vec![
                i32_node_with_value("int_speed", 12),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("int_speed", "value", "shader_1", "speed")],
        );
        let int_scene =
            render_scene_from_preview_document(&int_document).expect("scene should build");
        assert_eq!(shader_u_value(&int_scene), 12.0);

        let uint_document = document_with_edges(
            vec![
                u32_node_with_value("uint_speed", 7),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("uint_speed", "value", "shader_1", "speed")],
        );
        let uint_scene =
            render_scene_from_preview_document(&uint_document).expect("scene should build");
        assert_eq!(shader_u_value(&uint_scene), 7.0);
    }

    #[test]
    fn fullscreen_shader_converts_float_source_to_uint_uniform() {
        let document = document_with_edges(
            vec![
                value_node_with_id_and_value("float_count", json!(12.9)),
                shader_node(json!("wgsl"), json!(uint_shader_source())),
            ],
            vec![edge("float_count", "value", "shader_1", "count")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(
            shader_uniform(&scene, "count"),
            &ShaderUniformValue::U32(12)
        );
    }

    #[test]
    fn shader_uniform_value_defaults_unknown_uniform_data_kind() {
        let document =
            document_with_nodes(vec![shader_node(json!("wgsl"), json!(shader_source()))]);
        let node = document
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "shader_1")
            .expect("shader node should exist");
        let uniform: crate::ShaderUniform = serde_json::from_value(json!({
            "id": "unknown",
            "label": "Unknown",
            "type": { "flow": "control", "dataKind": "unknown.kind" },
            "required": false
        }))
        .expect("uniform should parse");

        assert_eq!(
            shader_uniform_value(&document, node, &uniform),
            ShaderUniformValue::F32(0.0)
        );
    }

    #[test]
    fn fullscreen_shader_reads_connected_value_node() {
        let document = document_with_edges(
            vec![
                value_node_with_value(json!(0.42)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "speed")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.42);
    }

    #[test]
    fn fullscreen_shader_reads_runtime_control_state_instead_of_graph_params() {
        let mut document = document_with_edges(
            vec![
                value_node_with_value(json!(0.42)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_1", "value", "shader_1", "speed")],
        );
        document
            .control_state
            .values
            .insert("value_1".to_owned(), ControlValue::float(2.5));

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 2.5);
    }

    #[test]
    fn fullscreen_shader_reads_connected_second_value_node() {
        let document = document_with_edges(
            vec![
                value_node_with_id_and_value("value_2", json!(0.73)),
                shader_node(json!("wgsl"), json!(shader_source())),
            ],
            vec![edge("value_2", "value", "shader_1", "phase")],
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
            vec![edge("color_1", "value", "shader_1", "tint")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_color(&scene), [1.0, 0.5, 0.0, 0.8]);
    }

    #[test]
    fn fullscreen_shader_preserves_connected_f32_value_without_global_range_clamp() {
        for (value, expected) in [(json!(-0.25), -0.25), (json!(1.25), 1.25)] {
            let document = document_with_edges(
                vec![
                    value_node_with_value(value),
                    shader_node(json!("wgsl"), json!(shader_source())),
                ],
                vec![edge("value_1", "value", "shader_1", "speed")],
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
            vec![edge("clear_1", "out", "shader_1", "speed")],
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
            vec![edge("value_1", "value", "shader_1", "tint")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_color(&scene), DEFAULT_SHADER_COLOR);
    }

    #[test]
    fn fullscreen_shader_defaults_u_value_for_missing_source_node() {
        let document = document_with_edges(
            vec![shader_node(json!("wgsl"), json!(shader_source()))],
            vec![edge("missing_value", "value", "shader_1", "speed")],
        );

        let scene = render_scene_from_preview_document(&document).expect("scene should build");

        assert_eq!(shader_u_value(&scene), 0.0);
    }

    #[test]
    fn fullscreen_shader_defaults_u_color_for_missing_source_node() {
        let document = document_with_edges(
            vec![shader_node(json!("wgsl"), json!(shader_source()))],
            vec![edge("missing_color", "value", "shader_1", "tint")],
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
            vec![edge("value_1", "value", "shader_1", "speed")],
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
                vec![edge("color_1", "value", "shader_1", "tint")],
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
            vec![edge("value_1", "value", "shader_1", "speed")],
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
    #[should_panic(expected = "expected f32 speed uniform")]
    fn shader_u_value_helper_rejects_wrong_uniform_type() {
        let scene = shader_scene_with_uniform("speed", ShaderUniformValue::Bool(false));

        let _ = shader_u_value(&scene);
    }

    #[test]
    #[should_panic(expected = "expected f32 phase uniform")]
    fn shader_u_value2_helper_rejects_wrong_uniform_type() {
        let scene = shader_scene_with_uniform("phase", ShaderUniformValue::Bool(false));

        let _ = shader_u_value2(&scene);
    }

    #[test]
    #[should_panic(expected = "expected color tint uniform")]
    fn shader_u_color_helper_rejects_wrong_uniform_type() {
        let scene = shader_scene_with_uniform("tint", ShaderUniformValue::Bool(false));

        let _ = shader_u_color(&scene);
    }

    #[test]
    #[should_panic(expected = "missing shader uniform missing")]
    fn shader_uniform_helper_rejects_missing_uniform() {
        let scene = shader_scene_with_uniform("speed", ShaderUniformValue::F32(0.0));

        let _ = shader_uniform(&scene, "missing");
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
                source_kind: "object.core.float".to_owned()
            }
        );
    }

    #[test]
    fn render_scene_build_errors_emit_shader_issues() {
        let cases = vec![
            (
                RenderSceneBuildError::ShaderInterfacePortsOutOfSync {
                    node_id: "shader_1".to_owned(),
                },
                "shader-interface-ports-out-of-sync",
                ShaderIssuePhase::SourceSync,
                ShaderIssueSource::Runtime,
            ),
            (
                RenderSceneBuildError::MissingShaderEntrypoint {
                    node_id: "shader_1".to_owned(),
                    entrypoint: "fs_main",
                },
                "missing-shader-entrypoint",
                ShaderIssuePhase::WgslGeneration,
                ShaderIssueSource::User,
            ),
            (
                RenderSceneBuildError::ReservedShaderEntrypoint {
                    node_id: "shader_1".to_owned(),
                    entrypoint: "vs_main",
                },
                "reserved-shader-entrypoint",
                ShaderIssuePhase::WgslGeneration,
                ShaderIssueSource::User,
            ),
            (
                RenderSceneBuildError::MissingShaderLanguage {
                    node_id: "shader_1".to_owned(),
                },
                "missing-shader-language",
                ShaderIssuePhase::SourceSync,
                ShaderIssueSource::User,
            ),
            (
                RenderSceneBuildError::UnsupportedShaderLanguage {
                    node_id: "shader_1".to_owned(),
                    language: "glsl".to_owned(),
                },
                "unsupported-shader-language",
                ShaderIssuePhase::SourceSync,
                ShaderIssueSource::User,
            ),
            (
                RenderSceneBuildError::MissingShaderSource {
                    node_id: "shader_1".to_owned(),
                },
                "missing-shader-source",
                ShaderIssuePhase::SourceSync,
                ShaderIssueSource::User,
            ),
            (
                RenderSceneBuildError::RenderOutputWithoutInput {
                    node_id: "output_1".to_owned(),
                },
                "render-output-without-input",
                ShaderIssuePhase::RenderPipeline,
                ShaderIssueSource::Runtime,
            ),
            (
                RenderSceneBuildError::MissingRenderOutputSourceNode {
                    output_node_id: "output_1".to_owned(),
                    source_node_id: "missing".to_owned(),
                },
                "missing-render-output-source-node",
                ShaderIssuePhase::RenderPipeline,
                ShaderIssueSource::Runtime,
            ),
            (
                RenderSceneBuildError::MissingRenderOutputSourcePort {
                    output_node_id: "output_1".to_owned(),
                    source_node_id: "shader_1".to_owned(),
                    port_id: "out".to_owned(),
                },
                "missing-render-output-source-port",
                ShaderIssuePhase::RenderPipeline,
                ShaderIssueSource::Runtime,
            ),
            (
                RenderSceneBuildError::UnsupportedRenderOutputSource {
                    output_node_id: "output_1".to_owned(),
                    source_node_id: "value_1".to_owned(),
                    source_kind: "object.core.float".to_owned(),
                },
                "unsupported-render-output-source",
                ShaderIssuePhase::RenderPipeline,
                ShaderIssueSource::Runtime,
            ),
        ];

        for (error, code, phase, source) in cases {
            let issues = error.shader_issues();

            assert_eq!(issues.len(), 1);
            assert_eq!(issues[0].code, code);
            assert_eq!(issues[0].phase, phase);
            assert_eq!(issues[0].source, source);
            assert!(!issues[0].message.is_empty());
        }

        let issue = ShaderIssue::error(
            ShaderIssuePhase::InterfaceAnalysis,
            "invalid-interface",
            "bad uniform",
            ShaderIssueSource::User,
        );
        let error = RenderSceneBuildError::InvalidShaderInterface {
            node_id: "shader_1".to_owned(),
            message: "bad uniform".to_owned(),
            issues: vec![issue.clone()],
        };

        assert_eq!(error.shader_issues(), vec![issue]);
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
        let graph = GraphDocument {
            schema: "skenion.graph".to_owned(),
            schema_version: "0.1.0".to_owned(),
            id: "render-graph".to_owned(),
            revision: "1".to_owned(),
            nodes,
            edges,
        };
        let control_state = crate::ControlState::from_graph(&graph);

        PreviewDocument {
            schema: PREVIEW_DOCUMENT_SCHEMA.to_owned(),
            schema_version: PREVIEW_DOCUMENT_SCHEMA_VERSION.to_owned(),
            graph,
            plan: ExecutionPlan {
                graph_id: "render-graph".to_owned(),
                graph_revision: "1".to_owned(),
                nodes: Vec::new(),
                edges: Vec::new(),
                groups: Vec::new(),
            },
            control_state,
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
        let ports = params
            .get("source")
            .and_then(Value::as_str)
            .map(analyze_shader_interface_v01)
            .filter(|analysis| analysis.ok)
            .map(|analysis| shader_interface_to_ports_v01(&analysis.shader_interface))
            .unwrap_or_else(|| vec![gpu_output_port()]);
        GraphNode {
            id: "shader_1".to_owned(),
            kind: RENDER_FULLSCREEN_SHADER_KIND.to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports,
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
            kind: "object.core.float".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "in",
                    "direction": "input",
                    "label": "In",
                    "type": {
                        "flow": "control", "dataKind": "value.core.message"
                    },
                    "required": false,
                    "activation": "trigger"
                }))
                .expect("valid value input port"),
                serde_json::from_value(json!({
                    "id": "cold",
                    "direction": "input",
                    "label": "Cold",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.float32"
                    },
                    "required": false,
                    "activation": "latched"
                }))
                .expect("valid value cold port"),
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Value",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.float32"
                    }
                }))
                .expect("valid value port"),
            ],
        }
    }

    fn i32_node_with_value(id: &str, value: i32) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("value".to_owned(), json!(value));
        GraphNode {
            id: id.to_owned(),
            kind: "object.core.int".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Value",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.int32"
                    }
                }))
                .expect("valid i32 value port"),
            ],
        }
    }

    fn bool_payload_source_node(id: &str) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            kind: "object.core.message".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params: serde_json::Map::new(),
            ports: vec![
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Value",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.bool"
                    }
                }))
                .expect("valid bool value port"),
            ],
        }
    }

    fn u32_node_with_value(id: &str, value: u64) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("value".to_owned(), json!(value));
        params.insert("representation".to_owned(), json!("u32"));
        GraphNode {
            id: id.to_owned(),
            kind: "object.core.int".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Value",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.uint32"
                    }
                }))
                .expect("valid u32 value port"),
            ],
        }
    }

    fn color_node_with_value(value: Value) -> GraphNode {
        let mut params = serde_json::Map::new();
        params.insert("value".to_owned(), value);
        GraphNode {
            id: "color_1".to_owned(),
            kind: "object.core.color".to_owned(),
            kind_version: "0.1.0".to_owned(),
            params,
            ports: vec![
                serde_json::from_value(json!({
                    "id": "in",
                    "direction": "input",
                    "label": "In",
                    "type": {
                        "flow": "control", "dataKind": "value.core.message"
                    },
                    "required": false,
                    "activation": "trigger"
                }))
                .expect("valid color input port"),
                serde_json::from_value(json!({
                    "id": "cold",
                    "direction": "input",
                    "label": "Cold",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.color"
                    },
                    "required": false,
                    "activation": "latched"
                }))
                .expect("valid color cold port"),
                serde_json::from_value(json!({
                    "id": "value",
                    "direction": "output",
                    "label": "Color",
                    "type": {
                        "flow": "control",
                        "dataKind": "value.core.color"
                    }
                }))
                .expect("valid color port"),
            ],
        }
    }

    fn shader_u_value(scene: &RenderScene) -> f32 {
        match shader_uniform(scene, "speed") {
            ShaderUniformValue::F32(value) => *value,
            _ => panic!("expected f32 speed uniform"),
        }
    }

    fn shader_u_value2(scene: &RenderScene) -> f32 {
        match shader_uniform(scene, "phase") {
            ShaderUniformValue::F32(value) => *value,
            _ => panic!("expected f32 phase uniform"),
        }
    }

    fn shader_u_color(scene: &RenderScene) -> [f32; 4] {
        match shader_uniform(scene, "tint") {
            ShaderUniformValue::ColorRgba(value) => *value,
            _ => panic!("expected color tint uniform"),
        }
    }

    fn shader_uniform<'a>(scene: &'a RenderScene, id: &str) -> &'a ShaderUniformValue {
        match scene {
            RenderScene::FullscreenShader(shader) => {
                &shader
                    .uniforms
                    .iter()
                    .find(|uniform| uniform.id == id)
                    .unwrap_or_else(|| panic!("missing shader uniform {id}"))
                    .value
            }
            _ => panic!("expected fullscreen shader scene"),
        }
    }

    fn shader_scene_with_uniform(id: &str, value: ShaderUniformValue) -> RenderScene {
        RenderScene::FullscreenShader(FullscreenShaderScene {
            language: ShaderLanguage::Wgsl,
            source: "fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }".to_owned(),
            source_node_id: "shader_1".to_owned(),
            uniforms: vec![ShaderUniformBinding {
                id: id.to_owned(),
                value,
            }],
            fallback_clear_color: DEFAULT_CLEAR_COLOR,
        })
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
            "dataKind": "value.core.tensor",
            "format": "rgba8unorm",
            "colorSpace": "srgb"
        })
    }

    fn shader_source() -> &'static str {
        r#"// @skenion.uniform speed value.core.float32 default=0 min=0 max=1 step=0.01
// @skenion.uniform phase value.core.float32 default=0 min=0 max=1 step=0.01
// @skenion.uniform tint value.core.color default=[1,1,1,1]
@fragment
fn fs_main() -> @location(0) vec4<f32> {
  let mix_value = clamp(skenion.speed, 0.0, 1.0);
  let brightness = 0.25 + 0.75 * clamp(skenion.phase, 0.0, 1.0);
  let animated = vec3<f32>(0.2 + mix_value * 0.8, 0.3, 1.0 - mix_value);
  return vec4<f32>(mix(animated, skenion.tint.rgb, mix_value) * brightness, skenion.tint.a);
}"#
    }

    fn typed_shader_source() -> &'static str {
        r#"// @skenion.uniform iterations value.core.int32 default=8
// @skenion.uniform enabled value.core.bool default=true
@fragment
fn fs_main() -> @location(0) vec4<f32> {
  let enabled_value = select(0.0, 1.0, skenion.enabled);
  return vec4<f32>(f32(skenion.iterations) / 16.0, enabled_value, 0.25, 1.0);
}"#
    }

    fn uint_shader_source() -> &'static str {
        r#"// @skenion.uniform count value.core.uint32 default=4
@fragment
fn fs_main() -> @location(0) vec4<f32> {
  return vec4<f32>(f32(skenion.count) / 255.0, 0.0, 0.0, 1.0);
}"#
    }
}
