use std::{
    error::Error,
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};

use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::{
    ExecutionPlan,
    preview_manager::{PreviewHandle, PreviewSpawn},
    render::{PreviewDocument, write_preview_document},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewFrameLimit {
    Frames(usize),
    UntilClose,
}

pub fn run_preview_window(
    plan: ExecutionPlan,
    frame_limit: PreviewFrameLimit,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = PreviewApp::new(plan, frame_limit);
    event_loop.run_app(&mut app)?;
    Ok(())
}

pub(crate) fn spawn_preview_document_handle(
    document: &PreviewDocument,
    telemetry_path: &Path,
    control_state_path: &Path,
) -> Result<PreviewSpawn, String> {
    let document_path = write_preview_document(document)?;
    let child = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg("preview-document")
        .arg("--document")
        .arg(&document_path)
        .arg("--until-close")
        .arg("--telemetry")
        .arg(telemetry_path)
        .arg("--control-state")
        .arg(control_state_path)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(PreviewSpawn::new(
        Box::new(ChildPreviewHandle { child }),
        Some(document_path),
    ))
}

struct PreviewApp {
    plan: ExecutionPlan,
    window: Option<Window>,
    frame_index: usize,
    frame_limit: PreviewFrameLimit,
    last_tick: Instant,
}

impl PreviewApp {
    fn new(plan: ExecutionPlan, frame_limit: PreviewFrameLimit) -> Self {
        Self {
            plan,
            window: None,
            frame_index: 0,
            frame_limit,
            last_tick: Instant::now(),
        }
    }

    fn window_title(&self) -> String {
        format!(
            "skenion preview - {} frame {}{} nodes {}",
            self.plan.graph_id,
            self.frame_index,
            self.frame_limit_label(),
            self.plan.nodes.len()
        )
    }

    fn frame_limit_label(&self) -> String {
        match self.frame_limit {
            PreviewFrameLimit::Frames(frame_count) => format!("/{frame_count}"),
            PreviewFrameLimit::UntilClose => " until close".to_owned(),
        }
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

impl ApplicationHandler for PreviewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attributes = WindowAttributes::default()
            .with_title(self.window_title())
            .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
        match event_loop.create_window(attributes) {
            Ok(window) => {
                self.window = Some(window);
                self.request_redraw();
            }
            Err(error) => {
                eprintln!("failed to create preview window: {error}");
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
            WindowEvent::RedrawRequested => {
                window.set_title(&self.window_title());
                self.frame_index += 1;
                if self.should_exit_after_redraw() {
                    event_loop.exit();
                } else {
                    self.last_tick = Instant::now();
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self.last_tick.elapsed() >= Duration::from_millis(16) {
            self.request_redraw();
        }
    }
}

struct ChildPreviewHandle {
    child: Child,
}

impl PreviewHandle for ChildPreviewHandle {
    fn pid(&self) -> Option<u32> {
        Some(self.child.id())
    }

    fn try_wait(&mut self) -> Result<Option<i32>, String> {
        self.child
            .try_wait()
            .map(|status| status.map(exit_code))
            .map_err(|error| error.to_string())
    }

    fn stop(&mut self) -> Result<Option<i32>, String> {
        self.child.kill().map_err(|error| error.to_string())?;
        self.child
            .wait()
            .map(|status| Some(exit_code(status)))
            .map_err(|error| error.to_string())
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}
