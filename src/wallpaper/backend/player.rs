use std::ffi::c_void;

use libmpv2::Mpv;
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use log::error;

use super::BackendCtx;
use super::transition::Transition;
use crate::config::PlayerConfig;

fn get_proc_address(egl_instance: &khronos_egl::Instance<khronos_egl::Static>, name: &str) -> *mut c_void {
    egl_instance
        .get_proc_address(name)
        .map(|f| f as *mut c_void)
        .unwrap_or(std::ptr::null_mut())
}

/// One mpv instance plus its OpenGL render context.
/// The `Mpv` is intentionally leaked to obtain a static ref.
struct MpvInstance {
    mpv: &'static Mpv,
    render: Option<RenderContext<'static>>,
    path: String,
}

impl MpvInstance {
    fn new(config: &PlayerConfig, debug: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let mpv = Box::leak(Box::new(Mpv::with_initializer(|init| {
            init.set_option("vo", "libmpv")
        })?));

        if debug || std::env::var("LP_DEBUG").is_ok() {
            mpv.set_property("terminal", "yes")?;
            mpv.set_property("msg-level", "all=status")?;
        }
        mpv.set_property("loop-file", "inf")?;
        mpv.set_property("hwdec", config.hwdec.as_str())?;
        mpv.set_property("mute", config.mute)?;
        mpv.set_property("speed", config.speed)?;
        mpv.set_property("panscan", if config.fill { 1.0 } else { 0.0 })?;

        for (name, value) in &config.mpv_options {
            if let Err(e) = mpv.set_property(name.as_str(), value.as_str()) {
                error!("Failed to set mpv option {name}={value}: {e}");
            }
        }

        Ok(Self {
            mpv,
            render: None,
            path: String::new(),
        })
    }

    fn init(&mut self, ctx: &BackendCtx) -> Result<(), Box<dyn std::error::Error>> {
        let render = self.mpv.create_render_context(vec![
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams {
                get_proc_address,
                ctx: khronos_egl::Instance::new(khronos_egl::Static),
            }),
            RenderParam::WaylandDisplay(ctx.display_ptr as *const c_void),
        ])?;
        self.render = Some(render);
        Ok(())
    }

    fn loadfile(&mut self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        if !path.contains("://") && !std::path::Path::new(path).exists() {
            return Err(format!("video file not found: {path}").into());
        }
        self.mpv.command("loadfile", &[path])?;
        self.path = path.to_string();
        Ok(())
    }

    fn stop(&mut self) {
        let _ = self.mpv.command("stop", &[]);
        self.path.clear();
    }

    fn render(&mut self, fbo: i32, width: i32, height: i32) {
        if let Some(render) = &self.render {
            let _ = render.render::<()>(fbo, width, height, true);
        }
    }

    fn pause(&mut self) {
        if let Err(e) = self.mpv.set_property("pause", true) {
            error!("Failed to pause mpv: {e}");
        }
    }

    fn resume(&mut self) {
        if let Err(e) = self.mpv.set_property("pause", false) {
            error!("Failed to resume mpv: {e}");
        }
    }

    fn set_speed(&mut self, speed: f64) -> Result<(), Box<dyn std::error::Error>> {
        self.mpv.set_property("speed", speed)?;
        Ok(())
    }

    fn set_mute(&mut self, mute: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.mpv.set_property("mute", mute)?;
        Ok(())
    }

    fn set_fill(&mut self, fill: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.mpv.set_property("panscan", if fill { 1.0 } else { 0.0 })?;
        Ok(())
    }

    fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Clone, Copy)]
enum PlayerState {
    Playing,
    Crossfading { start_ms: u32, duration_ms: u32 },
}

/// Wraps two mpv instances and crossfades between them on video switches.
pub struct Player {
    current: MpvInstance,
    next: MpvInstance,
    transition: Option<Transition>,
    state: PlayerState,
    config: PlayerConfig,
    display_ptr: *mut c_void,
    debug: bool,
    pending_path: Option<String>,
}

impl Player {
    pub fn new(
        path: impl ToString,
        config: &PlayerConfig,
        debug: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.to_string();
        let mut current = MpvInstance::new(config, debug)?;
        current.loadfile(&path)?;

        Ok(Self {
            current,
            next: MpvInstance::new(config, debug)?,
            transition: None,
            state: PlayerState::Playing,
            config: config.clone(),
            display_ptr: std::ptr::null_mut(),
            debug,
            pending_path: None,
        })
    }

    pub fn init(&mut self, ctx: BackendCtx) -> Result<(), Box<dyn std::error::Error>> {
        self.display_ptr = ctx.display_ptr;
        self.current.init(&ctx)?;
        self.next.init(&ctx)?;
        self.transition = Some(Transition::new(ctx.gl, 1, 1));
        Ok(())
    }

    pub fn render(&mut self, gl: &glow::Context, width: i32, height: i32, time: u32) {
        if self.transition.is_none() {
            self.transition = Some(Transition::new(gl, width, height));
        } else {
            self.transition.as_mut().unwrap().resize(gl, width, height);
        }
        let transition = self.transition.as_ref().unwrap();

        // Handle a pending switch by starting the crossfade now.
        if let Some(path) = self.pending_path.take() {
            if let Err(e) = self.next.loadfile(&path) {
                error!("Failed to load next video for crossfade: {e}");
                return;
            }
            transition.clear_next(gl);
            self.state = PlayerState::Crossfading {
                start_ms: time,
                duration_ms: self.config.crossfade_duration_ms,
            };
        }

        match self.state {
            PlayerState::Playing => {
                self.current.render(transition.current_fbo(), width, height);
                transition.render_blend(gl, 0.0, width, height);
            }
            PlayerState::Crossfading { start_ms, duration_ms } => {
                let alpha = ((time.saturating_sub(start_ms)) as f32 / duration_ms as f32)
                    .clamp(0.0, 1.0);

                self.current.render(transition.current_fbo(), width, height);
                self.next.render(transition.next_fbo(), width, height);
                transition.render_blend(gl, alpha, width, height);

                if alpha >= 1.0 {
                    // Swap: next becomes current, and clear the old current.
                    std::mem::swap(&mut self.current, &mut self.next);
                    self.next.stop();
                    self.state = PlayerState::Playing;
                }
            }
        }
    }

    pub fn pause(&mut self) {
        self.current.pause();
        self.next.pause();
    }

    pub fn resume(&mut self) {
        self.current.resume();
        self.next.resume();
    }

    /// Swap the playing file with a crossfade.
    pub fn set_video(&mut self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        if path == self.current.path() || path == self.pending_path.as_deref().unwrap_or("") {
            return Ok(());
        }
        self.pending_path = Some(path.to_string());
        Ok(())
    }

    pub fn set_speed(&mut self, speed: f64) -> Result<(), Box<dyn std::error::Error>> {
        self.config.speed = speed;
        self.current.set_speed(speed)?;
        self.next.set_speed(speed)?;
        Ok(())
    }

    pub fn set_mute(&mut self, mute: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.config.mute = mute;
        self.current.set_mute(mute)?;
        self.next.set_mute(mute)?;
        Ok(())
    }

    pub fn set_fill(&mut self, fill: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.config.fill = fill;
        self.current.set_fill(fill)?;
        self.next.set_fill(fill)?;
        Ok(())
    }

    pub fn path(&self) -> &str {
        self.current.path()
    }
}
