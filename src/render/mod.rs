mod native;
mod preview_document;
mod scene;

pub(crate) use native::generated_shader_response_from_preview_document;
pub use native::{
    GeneratedShaderResponse, GeneratedShaderSource, GeneratedShaderSourceMap,
    run_render_preview_document_file,
};
#[cfg(test)]
pub(crate) use preview_document::{PREVIEW_DOCUMENT_SCHEMA, PREVIEW_DOCUMENT_SCHEMA_VERSION};
pub(crate) use preview_document::{
    PreviewDocument, cleanup_stale_preview_temp_files, remove_preview_temp_file,
    write_preview_document,
};
pub(crate) use scene::render_scene_from_preview_document;
pub use scene::{
    ClearColorScene, DEFAULT_CLEAR_COLOR, FullscreenShaderScene, RENDER_CLEAR_COLOR_KIND,
    RENDER_FULLSCREEN_SHADER_KIND, RENDER_OUTPUT_KIND, RenderScene, RenderSceneBuildError,
    ShaderLanguage, ShaderUniformBinding, ShaderUniformValue,
};
