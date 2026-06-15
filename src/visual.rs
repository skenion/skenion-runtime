use std::{
    error::Error,
    time::{Duration, Instant},
};

use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::ExecutionPlan;

pub fn run_preview_window(plan: ExecutionPlan, frame_count: usize) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut app = PreviewApp::new(plan, frame_count.max(1));
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct PreviewApp {
    plan: ExecutionPlan,
    window: Option<Window>,
    frame_index: usize,
    frame_count: usize,
    last_tick: Instant,
}

impl PreviewApp {
    fn new(plan: ExecutionPlan, frame_count: usize) -> Self {
        Self {
            plan,
            window: None,
            frame_index: 0,
            frame_count,
            last_tick: Instant::now(),
        }
    }

    fn window_title(&self) -> String {
        format!(
            "Skenion Preview - {} frame {}/{} nodes {}",
            self.plan.graph_id,
            self.frame_index.min(self.frame_count),
            self.frame_count,
            self.plan.nodes.len()
        )
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
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
                if self.frame_index >= self.frame_count {
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
