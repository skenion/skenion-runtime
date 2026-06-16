use std::{error::Error, sync::Arc, time::Instant};

use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    PreviewFrameLimit,
    render::{PreviewDocument, RenderScene, render_scene_from_preview_document},
};

pub fn run_render_preview_window(
    document: PreviewDocument,
    frame_limit: PreviewFrameLimit,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let scene = render_scene_from_preview_document(&document);
    let mut app = NativePreviewApp::new(document, scene, frame_limit);
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct NativePreviewApp {
    document: PreviewDocument,
    scene: RenderScene,
    window: Option<Arc<Window>>,
    renderer: Option<WgpuClearRenderer>,
    frame_index: usize,
    frame_limit: PreviewFrameLimit,
    last_redraw: Instant,
}

impl NativePreviewApp {
    fn new(document: PreviewDocument, scene: RenderScene, frame_limit: PreviewFrameLimit) -> Self {
        Self {
            document,
            scene,
            window: None,
            renderer: None,
            frame_index: 0,
            frame_limit,
            last_redraw: Instant::now(),
        }
    }

    fn title(&self) -> String {
        let source = self
            .scene
            .source_node_id
            .as_deref()
            .unwrap_or("default-clear");
        format!(
            "Skenion Preview - {} rev {} session {} source {}",
            self.document.graph.id,
            self.document.graph.revision,
            self.document.session_revision,
            source
        )
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn should_exit_after_redraw(&self) -> bool {
        match self.frame_limit {
            PreviewFrameLimit::Frames(frame_count) => self.frame_index >= frame_count.max(1),
            PreviewFrameLimit::UntilClose => false,
        }
    }
}

impl ApplicationHandler for NativePreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default()
            .with_title(self.title())
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("failed to create preview window: {error}");
                event_loop.exit();
                return;
            }
        };

        match WgpuClearRenderer::new(Arc::clone(&window)) {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                self.window = Some(window);
                self.request_redraw();
            }
            Err(error) => {
                eprintln!("failed to initialize preview renderer: {error}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        if window.id() != window_id {
            return;
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                window.set_title(&self.title());
                if let Some(renderer) = &mut self.renderer
                    && let Err(error) = renderer.render(&self.scene)
                {
                    eprintln!("failed to render preview frame: {error}");
                }
                self.frame_index += 1;
                if self.should_exit_after_redraw() {
                    event_loop.exit();
                } else {
                    self.last_redraw = Instant::now();
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.last_redraw.elapsed() >= std::time::Duration::from_millis(16) {
            self.request_redraw();
        }
    }
}

struct WgpuClearRenderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl WgpuClearRenderer {
    fn new(window: Arc<Window>) -> Result<Self, String> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window)
            .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|error| error.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("skenion-preview-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|error| error.to_string())?;
        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or_else(|| "preview surface is not supported by the selected adapter".to_owned())?;
        surface.configure(&device, &config);

        Ok(Self {
            surface,
            device,
            queue,
            config,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.config.width == width && self.config.height == height {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn render(&mut self, scene: &RenderScene) -> Result<(), String> {
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(output)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
                    status => return Err(format!("surface acquisition retry failed: {status:?}")),
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("surface acquisition validation error".to_owned());
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("skenion-preview-clear-encoder"),
            });
        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("skenion-preview-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu_color(scene.clear_color)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit([encoder.finish()]);
        output.present();
        Ok(())
    }
}

fn wgpu_color(color: [f64; 4]) -> wgpu::Color {
    wgpu::Color {
        r: color[0],
        g: color[1],
        b: color[2],
        a: color[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scene_color_to_wgpu_color() {
        let color = wgpu_color([0.1, 0.2, 0.3, 1.0]);

        assert_eq!(color.r, 0.1);
        assert_eq!(color.g, 0.2);
        assert_eq!(color.b, 0.3);
        assert_eq!(color.a, 1.0);
    }
}
