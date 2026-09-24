// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! The evaluation seam (C++ `RenderProcessor : NodeTraverser`,
//! flattened): turns node-graph evaluation into render jobs by
//! implementing oaknode's `RenderHooks`. Each C++ `process_*` virtual
//! is one hook method.
//!
//! This pass implements the graph hooks: frame generation runs fully;
//! plugin jobs dispatch through the executor slot oakplugin installs
//! ([`set_plugin_executor`]); footage jobs decode through the oakcodec
//! decoder bridge; shader jobs execute on the shared GPU context
//! ([`oak_core::backend::GpuContext`], falling back to an input pass-
//! through when no adapter is available); color transform jobs apply
//! their OCIO processor (CPU frames convert for real; the GPU
//! color-managed blit is deferred at the backend and passes through);
//! cache jobs read their frame from the self-describing container in
//! [`crate::frameio`] (liboakoiio EXR/JPEG pending) and otherwise
//! substitute the value the cache node was fed. Resolution is one pass
//! over the output table ([`RenderHooks::resolve`]) that runs each
//! boxed [`oak_node::jobs::Job`] — and, first, the jobs nested in its
//! inputs — then replaces the box with the resulting texture.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::error::{Error, Result};
use crate::shaderfx::{compile_effect, run_effect};
use oak_codec::decoder::{
    CodecStream, RenderMode, RetrieveAudioStatus, RetrieveVideoParams, K_COLOR_RANGE_DEFAULT,
};
use oak_codec::ffmpeg::FFmpegDecoder;
use oak_core::frame::VideoParamsPod;
use oak_core::texture::{Frame, Texture};
use oak_core::{PixelFormat, Rational, TimeRange};
use oak_node::jobs::{
    CacheJobPayload, ColorTransformJobPayload, FootageJobPayload, Job, ShaderJobPayload,
};
use oak_node::nodes::plugin::PluginJobPayload;
use oak_node::value::{NodeValue, NodeValueRow, NodeValueTable};

/// Static mapping of OCIO-based node shaders to the OCIO function they
/// splice at their `%1` marker: (node type id, OCIO function name, source
/// space/role, destination space/role) — the C++ `GenerateProcessor`
/// `ColorTransform` pairs, resolved by the OCIO config at runtime.
///
/// Only the node's *function* (the GPU stub) is requested here; the
/// processor is built against the manager's reference color space
/// (C++ chromakey.cpp `GenerateProcessor` uses `GetReferenceColorSpace`,
/// which `ColorManager` sets to `OCIO::ROLE_SCENE_LINEAR`,
/// colormanager.cpp).
pub const OCIO_SHADER_STUBS: &[(&str, &str, &str, &str)] = &[(
    "org.olivevideoeditor.Olive.chromakey",
    "SceneLinearToCIEXYZ_d65",
    "scene_linear",
    "cie_xyz_d65_interchange",
)];

/// Static mapping of the OCIO grading nodes to the grading-primary style
/// whose dynamic GPU shader they splice at their `%1` marker (C++
/// `oakrender_color_processor_create_grading_primary` with the node's
/// `GRADING_LIN`/`GRADING_LOG` style; the processor is built against the
/// default config at render time).
pub const OCIO_GRADING_STUBS: &[(&str, oak_core::color::GradingStyle)] = &[
    (
        "org.olivevideoeditor.Olive.ociogradingtransformlinear",
        oak_core::color::GradingStyle::Lin,
    ),
    (
        "org.olivevideoeditor.Olive.OCIO_NAMESPACEgradingtransformlog",
        oak_core::color::GradingStyle::Log,
    ),
];

/// The OCIO GPU function shader for `type_id` (the `%1` stub), or `None`
/// when the node is not OCIO-based or the processor cannot be generated
/// (no default config, or a LUT processor the renderer cannot upload).
pub fn ocio_stub_for(type_id: &str) -> Option<String> {
    let (_, fn_name, from, to) = OCIO_SHADER_STUBS
        .iter()
        .find(|(id, ..)| *id == type_id)
        .copied()?;
    oak_core::color::ocio_function_shader(fn_name, from, to)
}

/// The OCIO grading GPU shader for `type_id` (the `%1` stub), or `None`
/// when the node is not a grading node or no default config is set up.
pub fn grading_stub_for(type_id: &str) -> Option<String> {
    let (_, style) = OCIO_GRADING_STUBS
        .iter()
        .find(|(id, _)| *id == type_id)
        .copied()?;
    oak_core::color::grading_primary_function_shader(style)
}

/// Job specification: the closed set of C++ `*Job` payloads
/// (AcceleratedJob family) as internal evaluation records — jobs no
/// longer travel inside values across module boundaries.
#[derive(Clone, Debug)]
pub enum JobSpec {
    /// Shader job (frag/vert source + params).
    Shader {
        /// Fragment source.
        frag: String,
        /// Vertex source.
        vert: String,
    },
    /// Color transform job.
    ColorTransform {
        /// Processor identity (color::ProcessorCache key).
        processor: u64,
    },
    /// Direct frame generation (CPU nodes).
    Generate,
    /// Footage decode (C++ FootageJob; decode via bridge::codec).
    Footage {
        /// Decoder/stream id.
        decoder_id: String,
        /// Footage filename (M12 P0: the decode path).
        filename: String,
        /// Media stream index.
        stream_index: i32,
    },
    /// Sample generation (C++ SampleJob).
    Sample,
    /// OFX plugin job — executed through the registered plugin executor
    /// ([`set_plugin_executor`]; the oakplugin crate installs its
    /// render driver there, so oakrender never sees OFX types).
    Plugin {
        /// Plugin instance identity (oakplugin instance registry key).
        instance: u64,
        /// OFX plugin identifier (cross-process stable). The single OFX
        /// host process (M3) resolves its own instance from this; the
        /// in-process executor ignores it and uses `instance`.
        type_id: String,
        /// Request time in seconds (C++ `PluginJob` time).
        time: f64,
        /// Clip name the main source texture arrives on (C++
        /// `node->get_effect_input_id()`).
        effect_input_id: Option<String>,
        /// Clip input textures by clip name (multi-input plugins).
        inputs: Vec<(String, Texture)>,
        /// Param overrides: input id -> node value (the tagged values
        /// captured at evaluation time).
        values: Vec<(String, NodeValue)>,
    },
}

/// The hooks implementation handed to the oaknode traverser.
pub struct RenderEvalHooks {
    /// Cache usage toggle (C++ use_cache).
    pub use_cache: bool,
    /// Active ticket identity (for cancellation polling).
    pub ticket: Option<crate::ticket::TicketId>,
    /// Forced output size for resolved footage jobs; `None` decodes at
    /// the media's native size (C++ `RenderProcessor` requests the
    /// texture at the output resolution). The graph-sequence driver sets
    /// this to the sequence frame size.
    pub frame_size: Option<(i32, i32)>,
    /// The sequence's square-pixel resolution (C++ the `NodeGlobals`
    /// square resolution the shape/polygon generators insert as
    /// `resolution_in` at job-build time). Generator jobs anchor their
    /// pixel-size params to this — NOT to the render-target size — so a
    /// proxy-resolution playback render draws them at the same relative
    /// size as a paused full-res frame. `None` (non-sequence renders)
    /// falls back to the render-target size.
    pub sequence_size: Option<(i32, i32)>,
    /// Position of the adjustment layer currently being swept, in
    /// `0.0..=1.0` (C++ the adjustment/transition `progress_in` uniform).
    /// [`render_graph_frame`] sets this for the duration of one adjustment
    /// block's evaluation, so a chain hanging off an adjustment layer can
    /// fade its result across the layer's span. Only shaders declaring a
    /// `progress_in` uniform consume it.
    pub layer_progress: Option<f64>,
}

// ---------------------------------------------------------------------------
// Plugin job executor (dependency inversion seam)
// ---------------------------------------------------------------------------
//
// oakrender sits BELOW oakplugin in the dependency graph (oakplugin
// depends on oakrender for the texture value types), so the plugin job
// execution cannot be a direct call. The oakplugin crate installs its
// render driver here at init; `process_plugin_job` dispatches through
// the slot. Without an executor, plugin jobs fail explainably (the
// pre-wiring behavior).

/// Plugin job request handed to the registered executor (the C++
/// `process_plugin_job(texture, destination, node)` inputs flattened).
pub struct PluginJobRequest<'a> {
    /// The job spec ([`JobSpec::Plugin`] guaranteed by the caller).
    pub spec: &'a JobSpec,
    /// The input texture the job runs against.
    pub src: Texture,
}

/// Plugin executor: runs one plugin job and returns the output
/// texture. Implemented by the oakplugin crate on top of its render
/// driver.
pub type PluginExecutor = dyn Fn(&PluginJobRequest<'_>) -> Result<Texture> + Send + Sync;

static PLUGIN_EXECUTOR: std::sync::OnceLock<std::sync::Mutex<Option<Arc<PluginExecutor>>>> =
    std::sync::OnceLock::new();

fn executor_slot() -> &'static std::sync::Mutex<Option<Arc<PluginExecutor>>> {
    PLUGIN_EXECUTOR.get_or_init(|| std::sync::Mutex::new(None))
}

/// Install the plugin job executor (oakplugin registration point;
/// `None` clears it).
pub fn set_plugin_executor(executor: Option<Arc<PluginExecutor>>) {
    *executor_slot().lock().unwrap_or_else(|e| e.into_inner()) = executor;
}

/// The installed plugin executor, if any.
pub fn plugin_executor() -> Option<Arc<PluginExecutor>> {
    executor_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Plugin instance factory: resolves an OFX plugin identifier to a live
/// instance-registry id, creating (and caching) the instance lazily.
/// Implemented by the oakplugin crate — the montage effect path carries
/// plugin identifiers, not instance ids (the montage is resolved from
/// the timeline in the main process; the instance lives in whichever
/// process renders the frame).
pub type PluginInstanceFactory = dyn Fn(&str) -> Option<u64> + Send + Sync;

static PLUGIN_INSTANCE_FACTORY: std::sync::OnceLock<
    std::sync::Mutex<Option<Arc<PluginInstanceFactory>>>,
> = std::sync::OnceLock::new();

fn instance_factory_slot() -> &'static std::sync::Mutex<Option<Arc<PluginInstanceFactory>>> {
    PLUGIN_INSTANCE_FACTORY.get_or_init(|| std::sync::Mutex::new(None))
}

/// Install the plugin instance factory (oakplugin registration point;
/// `None` clears it).
pub fn set_plugin_instance_factory(factory: Option<Arc<PluginInstanceFactory>>) {
    *instance_factory_slot().lock().unwrap_or_else(|e| e.into_inner()) = factory;
}

/// The installed plugin instance factory, if any.
pub fn plugin_instance_factory() -> Option<Arc<PluginInstanceFactory>> {
    instance_factory_slot()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// Box a resolved texture into the table's texture channel (the render
/// seam's output is the one place a texture legitimately travels as a
/// refcounted handle — [`oak_node::handle::get_checked`] probes against
/// `Texture` stay type-checked).
fn texture_value(texture: Texture) -> NodeValue {
    NodeValue::Texture(oak_node::handle::make_owned(texture))
}

/// The failure marker frame: solid magenta (1, 0, 1, 1) F32 RGBA —
/// the C++ plugin renderer paints failed plugin output purple so a
/// broken plugin is visible instead of silently black.
fn purple_frame(time: Rational, size: (i32, i32)) -> Texture {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let mut frame = match generate_frame(time, (w, h), PixelFormat::F32) {
        Ok(f) => f,
        Err(_) => return Texture::dummy(),
    };
    for pixel in frame.data.chunks_exact_mut(16) {
        for (i, v) in [1.0f32, 0.0, 1.0, 1.0].iter().enumerate() {
            pixel[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    Texture::wrap_frame(frame)
}

impl RenderEvalHooks {
    /// A hook set with every optional hook unset (`use_cache` off).
    pub fn new() -> Self {
        Self {
            use_cache: false,
            ticket: None,
            frame_size: None,
            sequence_size: None,
            layer_progress: None,
        }
    }

    /// C++ process_color_transform: apply the job's OCIO processor to the
    /// input texture. A CPU frame converts in place (the real OCIO
    /// `convert_frame`); a GPU input gets the same transform baked into a
    /// 3D LUT and applied by the GPU LUT pass (M2: color management is
    /// never skipped), with a readback+convert+re-upload fallback for
    /// exotic processors. An invalid processor passes the input through
    /// unchanged (C++ creates processors non-fatally); a non-texture input
    /// is `Error::Invalid`.
    fn process_color_transform_job(
        &mut self,
        payload: &ColorTransformJobPayload,
    ) -> Result<Texture> {
        let NodeValue::Texture(handle) = &payload.input else {
            return Err(Error::Invalid);
        };
        if handle.ctx.is_null() {
            return Err(Error::Invalid);
        }
        let tex = (unsafe { oak_node::handle::get_checked::<Texture>(handle) })
            .cloned()
            .ok_or(Error::Invalid)?;
        if !payload.color_processor.is_valid() {
            // No valid processor (no default config, LUT load failure):
            // pass the input through, mirroring the C++ non-fatal
            // processor creation.
            return Ok(tex);
        }
        let mut tex = tex;
        if let Texture::Cpu(frame) = &mut tex {
            // The CPU leg converts in place (real OCIO `convert_frame`).
            payload.color_processor.convert_frame(frame)?;
            return Ok(tex);
        }
        // GPU leg: bake the processor into a 3D LUT and run the GPU color
        // pass (no readback). Fall back to an explicit readback + CPU
        // conversion + re-upload when the processor cannot be baked or the
        // texture's context is not a real GPU context.
        let (token, ctx, width, height) = match &tex {
            Texture::Gpu {
                token,
                ctx,
                width,
                height,
                ..
            } => (*token, ctx.clone(), *width, *height),
            // Imported planar frames never reach node processing (the
            // footage path resolves them first); pass through defensively.
            Texture::Cpu(_) | Texture::Planar(_) => return Ok(tex),
        };
        let concrete = ctx
            .as_any()
            .and_then(|a| a.downcast_ref::<oak_core::backend::GpuContext>());
        if let Some(concrete) = concrete {
            if let Some(lut) = color_transform_lut(&payload.color_processor) {
                if let Ok(dst) = concrete.apply_color_lut(
                    token,
                    &payload.color_processor.cache_id(),
                    &lut,
                ) {
                    return Ok(Texture::gpu(
                        ctx,
                        dst,
                        width,
                        height,
                        PixelFormat::F32,
                    ));
                }
            }
            let mut frame = tex.to_frame()?;
            payload.color_processor.convert_frame(&mut frame)?;
            let dst = concrete.create_texture(frame.width, frame.height)?;
            concrete.upload(dst, &frame)?;
            return Ok(Texture::gpu(
                ctx,
                dst,
                frame.width,
                frame.height,
                PixelFormat::F32,
            ));
        }
        let mut frame = tex.to_frame()?;
        payload.color_processor.convert_frame(&mut frame)?;
        Ok(Texture::wrap_frame(frame))
    }

    /// C++ process_frame_generation: fill the destination with a generated
    /// F32 frame (transparent black for now).
    ///
    /// Only the `generation_fills_cpu_texture` unit test drives this today;
    /// the live eval path uses the shader/graph hooks instead.
    #[allow(dead_code)]
    fn process_frame_generation(
        &mut self,
        destination: &mut Texture,
        time: Rational,
    ) -> Result<()> {
        let Texture::Cpu(frame) = destination else {
            return Err(Error::Failed(
                "frame generation on GPU deferred: CPU path only this pass".into(),
            ));
        };
        let generated = generate_frame(time, (frame.width, frame.height), frame.format)?;
        frame.data = generated.data;
        frame.timestamp = time;
        Ok(())
    }

    /// C++ process_plugin_job: dispatch through the single OFX host
    /// process when one is installed (M3), else through the in-process
    /// plugin executor (the oakplugin render driver; dependency
    /// inversion). A missing host/executor or a failed render yields a
    /// purple failure frame instead of aborting the graph, matching
    /// pluginjob.cpp's fallback.
    fn process_plugin_job(&mut self, src: Texture, spec: &JobSpec) -> Result<Texture> {
        let JobSpec::Plugin {
            instance,
            type_id,
            time,
            effect_input_id,
            inputs,
            values,
        } = spec
        else {
            return Err(Error::Invalid);
        };
        let size = src.size();
        if let Some(host) = crate::ofxhost::client() {
            return match host.submit(spec, &src) {
                Ok(frame) => Ok(Texture::wrap_frame(frame)),
                Err(err) => {
                    eprintln!(
                        "OFX host job {type_id} (instance {instance}) at t={time}s failed: {err:#}"
                    );
                    Ok(purple_frame(Rational::from_double(*time), size))
                }
            };
        }
        let Some(executor) = plugin_executor() else {
            return Ok(purple_frame(Rational::from_double(*time), size));
        };
        let _ = (instance, type_id, effect_input_id, inputs, values);
        match executor(&PluginJobRequest { spec, src }) {
            Ok(texture) => Ok(texture),
            Err(err) => {
                eprintln!("plugin instance {instance} render at t={time}s failed: {err:#}");
                Ok(purple_frame(Rational::from_double(*time), size))
            }
        }
    }

    /// C++ process_video_cache_job: read the frame the cache wrote at
    /// `payload.path` through the disk frame-cache container
    /// ([`crate::frameio`]). A missing or unreadable file is an `Err` —
    /// the caller then substitutes the job's fallback value. The
    /// payload's request time is already spelled into `path` (the cache
    /// writes one file per frame), so the read itself ignores it.
    fn process_cache_job(&mut self, payload: &CacheJobPayload) -> Result<Texture> {
        let frame = crate::frameio::load_cache_frame(&payload.path)?;
        Ok(Texture::wrap_frame(frame))
    }

    /// Resolve one texture-channel value in place (the C++ JobEngine's
    /// per-value walk): a boxed [`Job`] recurses into its own inputs and
    /// runs, then the box is replaced by the resulting texture; a genuine
    /// texture — or any non-texture value — passes through untouched.
    /// `depth` bounds the nesting; `in_flight` holds the job boxes on the
    /// current walk so a job reachable from itself stops instead of
    /// recursing forever (the C++ `resolved_texture_cache_`
    /// de-duplication, kept as a path set so the same handle reached from
    /// two rows still resolves per row).
    fn resolve_value(
        &mut self,
        value: &mut NodeValue,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) {
        /// Job-nesting recursion ceiling (defensive; real graphs nest a
        /// generator job inside a merge job and stop there).
        const MAX_JOB_DEPTH: usize = 64;
        if depth >= MAX_JOB_DEPTH {
            return;
        }
        let NodeValue::Texture(handle) = value else {
            return;
        };
        if handle.ctx.is_null() {
            return;
        }
        let key = handle.ctx as usize;
        if !in_flight.insert(key) {
            return;
        }
        let job = (unsafe { oak_node::jobs::job_ref(handle) }).cloned();
        if let Some(job) = job {
            if let Some(resolved) = self.process_job(&job, depth + 1, in_flight) {
                *value = resolved;
            }
        }
        in_flight.remove(&key);
    }

    /// Run one resolved [`Job`] (the C++ `process_*` virtual, dispatched on
    /// the payload type). `None` when the job produced no replacement
    /// value — the row then keeps its box.
    fn process_job(
        &mut self,
        job: &Job,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) -> Option<NodeValue> {
        match job {
            Job::FootageJob(payload) => self.process_footage_job_value(payload),
            Job::ShaderJob(payload) => {
                Some(self.process_shader_job_value(payload, depth, in_flight))
            }
            Job::PluginJob(payload) => self.process_plugin_job_value(payload, depth, in_flight),
            Job::ColorTransformJob(payload) => {
                Some(self.process_color_transform_job_value(payload, depth, in_flight))
            }
            Job::CacheJob(payload) => {
                Some(self.process_cache_job_value(payload, depth, in_flight))
            }
        }
    }

    /// Resolve one footage job (C++ FootageJob processing in
    /// jobmanager.cpp): decode the frame at the job's request time and
    /// replace the box with the resulting texture. `None` on a decode
    /// failure — the row keeps its box, so the failure stays visible and
    /// is retried rather than cached as a hole.
    fn process_footage_job_value(&mut self, payload: &FootageJobPayload) -> Option<NodeValue> {
        let size = self.frame_size.unwrap_or((0, 0));
        match render_footage_frame(
            &payload.filename,
            payload.stream_index,
            payload.time,
            size,
            PixelFormat::F32,
        ) {
            Ok(texture) => Some(texture_value(texture)),
            Err(err) => {
                eprintln!("footage job decode failed: {err:#}");
                None
            }
        }
    }

    /// Resolve one shader job (C++ ShaderJob processing in jobmanager.cpp):
    /// recurse into the param row's job boxes (the generator layer of an
    /// `mrg` chain), execute the pass, and replace the box with the result
    /// texture. A failed or un-runnable job falls back to the effect input
    /// texture from the now-resolved param row (a pass-through — C++ leaves
    /// the failed shader's output as its input); a row without the effect
    /// input resolves to `NodeValue::None`.
    fn process_shader_job_value(
        &mut self,
        payload: &ShaderJobPayload,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) -> NodeValue {
        let mut payload = payload.clone();
        for value in payload.params.values_mut() {
            self.resolve_value(value, depth, in_flight);
        }
        match self.process_shader_job(&payload) {
            Some(texture) => texture_value(texture),
            None => payload
                .params
                .get(&payload.effect_input)
                .cloned()
                .unwrap_or(NodeValue::None),
        }
    }

    /// Resolve one plugin job (C++ JobEnginePlugin processing in
    /// jobmanager.cpp): recurse into the payload's tagged values, split
    /// them into clip input textures and scalar param overrides, then
    /// dispatch the render through the installed executor. A failed render
    /// leaves the box in the row (the executor has already reported it).
    fn process_plugin_job_value(
        &mut self,
        payload: &PluginJobPayload,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) -> Option<NodeValue> {
        let mut payload = payload.clone();
        for value in payload.values.values_mut() {
            self.resolve_value(value, depth, in_flight);
        }

        let mut inputs: Vec<(String, Texture)> = Vec::new();
        let mut values: Vec<(String, NodeValue)> = Vec::new();
        for (key, v) in payload.values.iter() {
            match v {
                NodeValue::Texture(h) if !h.ctx.is_null() => {
                    match unsafe { oak_node::handle::get_checked::<Texture>(h) }.cloned() {
                        Some(texture) => inputs.push((key.clone(), texture)),
                        None => eprintln!("plugin job input '{key}' is not a texture box"),
                    }
                }
                NodeValue::Texture(_) | NodeValue::None => {}
                other => values.push((key.clone(), other.clone())),
            }
        }

        // Fallback order mirrors pluginrenderer.cpp's effect input
        // resolution: the declared effect input, else the first
        // available clip texture.
        let effect_src = if payload.effect_input_id.is_empty() {
            None
        } else {
            inputs
                .iter()
                .find(|(key, _)| key == &payload.effect_input_id)
                .map(|(_, t)| t.clone())
        };
        let src = effect_src
            .or_else(|| inputs.first().map(|(_, t)| t.clone()))
            .unwrap_or_else(Texture::dummy);

        let spec = JobSpec::Plugin {
            instance: payload.instance.0,
            type_id: payload.type_id.clone(),
            time: payload.time.to_f64(),
            effect_input_id: if payload.effect_input_id.is_empty() {
                None
            } else {
                Some(payload.effect_input_id.clone())
            },
            inputs,
            values,
        };
        match self.process_plugin_job(src, &spec) {
            Ok(texture) => Some(texture_value(texture)),
            Err(err) => {
                eprintln!("plugin job resolve failed: {err:#}");
                None
            }
        }
    }

    /// Resolve one color transform job (C++ ColorTransformJob processing in
    /// jobmanager.cpp): recurse into the input value, apply the processor,
    /// and replace the box with the result. A failure falls back to the
    /// job's resolved input texture (a pass-through — the C++ renderer
    /// leaves the failed transform's output as its input).
    fn process_color_transform_job_value(
        &mut self,
        payload: &ColorTransformJobPayload,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) -> NodeValue {
        let mut payload = payload.clone();
        self.resolve_value(&mut payload.input, depth, in_flight);
        match self.process_color_transform_job(&payload) {
            Ok(texture) => texture_value(texture),
            Err(err) => {
                eprintln!("color transform job failed: {err:#}");
                payload.input.clone()
            }
        }
    }

    /// Resolve one cache job (C++ CacheJob processing in jobmanager.cpp):
    /// recurse into the fallback value first, then read the frame the cache
    /// wrote at the job's path. A missing or unreadable file substitutes
    /// the fallback — a real texture by then, not another job box — and is
    /// logged once per path (a cache miss repeats every frame).
    fn process_cache_job_value(
        &mut self,
        payload: &CacheJobPayload,
        depth: usize,
        in_flight: &mut HashSet<usize>,
    ) -> NodeValue {
        let mut payload = payload.clone();
        self.resolve_value(&mut payload.fallback, depth, in_flight);
        match self.process_cache_job(&payload) {
            Ok(texture) => texture_value(texture),
            Err(err) => {
                let key = format!("cache:{}", payload.path);
                if unsupported_warned().insert(key) {
                    eprintln!(
                        "cache job \"{}\" failed, using the cache node's input: {err:#}",
                        payload.path
                    );
                }
                (*payload.fallback).clone()
            }
        }
    }

    /// Execute one shader payload (C++ process_shader run by the render
    /// worker): compile the emitting behavior's fragment shader on the
    /// shared GPU context and run the requested iterations. **Every**
    /// texture-typed param is bound by its input id (C++ binds all
    /// sampler inputs — merge's base/blend, the keyers' garbage/core
    /// mattes, opacity's texture modulation); a param boxing a nested
    /// shader payload resolves recursively first (the C++
    /// AcceleratedJob chain — generator-over-base `mrg` jobs). CPU
    /// frames upload into scratch textures; the pass size comes from
    /// the effect input's texture (else the first bound texture, else
    /// the hook's frame size, else 1x1). `None` when the job cannot run
    /// (no GPU context, unknown node type, missing shader, or a
    /// compile/upload/run failure) — the caller then falls back to the
    /// effect input texture.
    fn process_shader_job(&self, payload: &ShaderJobPayload) -> Option<Texture> {
        self.process_shader_job_depth(payload, 0)
    }

    /// [`Self::process_shader_job`] with a recursion guard for nested
    /// payloads (generator-over-base chains nest at most 2 deep).
    fn process_shader_job_depth(&self, payload: &ShaderJobPayload, depth: u32) -> Option<Texture> {
        /// Nested-payload recursion ceiling (defensive; real graphs nest
        /// a generator job inside a merge job and stop there).
        const MAX_JOB_DEPTH: u32 = 8;

        // One log line per shader per process instead of one per frame.
        let warn = |reason: &str| {
            let key = format!("shader:{}:{}", payload.type_id, payload.shader_id);
            if unsupported_warned().insert(key) {
                eprintln!(
                    "shader job \"{}\" (shader \"{}\") failed: {reason}",
                    payload.type_id, payload.shader_id
                );
            }
        };

        let Some(ctx) = oak_core::backend::GpuContext::shared() else {
            warn("no GPU context");
            return None;
        };

        // The emitting node behavior: the type id selects the fragment
        // source (C++ `node->get_shader_code(shader_id)`).
        let Some((_, behavior)) =
            oak_node::factory::Factory::global().create_any(&payload.type_id)
        else {
            warn("unknown node type");
            return None;
        };

        // OCIO-based nodes splice the auto-generated OCIO function into
        // their `%1` marker (C++ `GetShaderCode({shader_id, stub})`, the
        // stub built in colormanagement.cpp `GetColorContext`). A stub
        // that cannot be generated — no default config, or a LUT
        // processor with no upload path — falls back to the effect input
        // pass-through.
        let ocio_entry = OCIO_SHADER_STUBS
            .iter()
            .find(|(id, ..)| *id == payload.type_id)
            .copied();
        let grading_entry = OCIO_GRADING_STUBS
            .iter()
            .find(|(id, _)| *id == payload.type_id)
            .copied();
        let glsl = match (grading_entry, ocio_entry) {
            (Some((_, style)), _) => {
                let Some(stub) = oak_core::color::grading_primary_function_shader(style) else {
                    return None;
                };
                match behavior.shader_code(&stub) {
                    Some(glsl) => glsl,
                    None => {
                        warn("shader not found");
                        return None;
                    }
                }
            }
            (None, Some((_, fn_name, from, to))) => {
                let Some(stub) = oak_core::color::ocio_function_shader(fn_name, from, to) else {
                    return None;
                };
                match behavior.shader_code(&stub) {
                    Some(glsl) => glsl,
                    None => {
                        warn("shader not found");
                        return None;
                    }
                }
            }
            (None, None) => match behavior.shader_code(&payload.shader_id) {
                Some(glsl) => glsl,
                None => {
                    warn("shader not found");
                    return None;
                }
            },
        };

        // Pipeline cache key: the type id plus the shader-variant id (the
        // OCIO stub text folds in too, so a config change recompiles
        // instead of reusing a stale variant).
        let spliced_ocio = grading_entry.is_some() || ocio_entry.is_some();
        let key = if spliced_ocio {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&glsl, &mut h);
            format!(
                "{}:{}:ocio:{}",
                payload.type_id,
                payload.shader_id,
                std::hash::Hasher::finish(&h)
            )
        } else {
            format!("{}:{}", payload.type_id, payload.shader_id)
        };
        let compiled = match compile_effect(&ctx, &key, &glsl, ctx.is_filterable()) {
            Ok(effect) => effect,
            Err(err) => {
                warn(&format!("compile failed: {err:#}"));
                return None;
            }
        };

        // Bind every texture-typed param by name: genuine texture boxes
        // bind directly (CPU frames upload into scratch first); nested
        // shader payloads (the generator layer of an `mrg` job) resolve
        // recursively. `scratch` holds the upload tokens created here;
        // `keepalive` holds the cloned `Texture`s — a `Texture::Gpu`
        // clone destroys its token on drop, so the clones must outlive
        // the pass. Both are released when the job finishes (input-token
        // destruction at job end matches the historical semantics).
        let mut inputs: Vec<(String, u64)> = Vec::new();
        let mut scratch: Vec<u64> = Vec::new();
        let mut keepalive: Vec<Texture> = Vec::new();
        let mut size: Option<(i32, i32)> = None;
        let bind = |key: &str,
                        value: &NodeValue,
                        inputs: &mut Vec<(String, u64)>,
                        scratch: &mut Vec<u64>,
                        keepalive: &mut Vec<Texture>,
                        size: &mut Option<(i32, i32)>|
         -> Option<()> {
            let NodeValue::Texture(handle) = value else {
                return None;
            };
            if handle.ctx.is_null() {
                return None;
            }
            let tex = (unsafe { oak_node::handle::get_checked::<Texture>(handle) })
                .cloned()
                .or_else(|| {
                    if depth >= MAX_JOB_DEPTH {
                        return None;
                    }
                    let nested = (unsafe { oak_node::jobs::shader_job(handle) }).cloned()?;
                    self.process_shader_job_depth(&nested, depth + 1)
                });
            let tex = tex?;
            let (token, tex_size) = match &tex {
                Texture::Gpu {
                    token,
                    width,
                    height,
                    ..
                } => (*token, (*width, *height)),
                Texture::Cpu(frame) => {
                    let token = match ctx.create_texture(frame.width, frame.height) {
                        Ok(t) => t,
                        Err(err) => {
                            warn(&format!("input texture: {err:#}"));
                            return None;
                        }
                    };
                    if let Err(err) = ctx.upload(token, frame) {
                        ctx.destroy_texture(token);
                        warn(&format!("input upload failed: {err:#}"));
                        return None;
                    }
                    scratch.push(token);
                    (token, (frame.width, frame.height))
                }
                // Resolved by the footage path; never a shader input.
                Texture::Planar(_) => {
                    warn("planar texture reached a shader job input (unresolved)");
                    return None;
                }
            };
            // The pass size follows the effect input's texture (C++ the
            // job's video params = the main input size); any other bound
            // texture sets it only when no effect input was seen.
            if size.is_none() || key == payload.effect_input {
                *size = Some(tex_size);
            }
            inputs.push((key.to_string(), token));
            keepalive.push(tex);
            Some(())
        };

        // The effect input binds first: `run_effect` falls back to
        // `inputs.first()` for the shader's first declared sampler.
        let effect_value = payload.params.get(&payload.effect_input).cloned();
        if let Some(value) = &effect_value {
            bind(
                &payload.effect_input,
                value,
                &mut inputs,
                &mut scratch,
                &mut keepalive,
                &mut size,
            );
        }
        for (key, value) in &payload.params {
            if key == &payload.effect_input {
                continue;
            }
            bind(key, value, &mut inputs, &mut scratch, &mut keepalive, &mut size);
        }
        // Generators bind no texture: render at the requested frame size
        // (the graph driver sets it to the sequence size); 1x1 only when
        // nobody knows better.
        let size = size.or(self.frame_size).unwrap_or((1, 1));

        // `resolution_in` anchors to the sequence square resolution, not
        // the render target (C++ inserts the NodeGlobals square
        // resolution into the job at build time): the node params it
        // denormalizes (shape size/pos, transform offsets, corner pin
        // points, drop shadow distance) are all sequence-pixel values,
        // so a proxy-size playback render must resolve them against the
        // same resolution as a paused full-res frame, or the effect
        // visibly changes size whenever the transport stops. Pre-filling
        // the row wins over `run_effect`'s frame-size auto-fill; a node
        // that inserted its own `resolution_in` keeps it.
        //
        // `progress_in` is the same pre-fill for the adjustment-layer
        // sweep: a shader declaring the uniform receives the layer
        // progress the graph driver recorded for this evaluation. The
        // row is only cloned when something actually needs inserting.
        let mut anchored_row;
        let needs_resolution = self.sequence_size.is_some()
            && !payload.params.contains_key("resolution_in")
            && compiled
                .translated
                .uniforms
                .iter()
                .any(|u| u.name == "resolution_in");
        let needs_progress = self.layer_progress.is_some()
            && !payload.params.contains_key("progress_in")
            && compiled
                .translated
                .uniforms
                .iter()
                .any(|u| u.name == "progress_in");
        let params = if needs_resolution || needs_progress {
            anchored_row = payload.params.clone();
            if needs_resolution {
                let (w, h) = self.sequence_size.unwrap();
                anchored_row.insert(
                    "resolution_in".to_string(),
                    NodeValue::Vec2([w as f64, h as f64]),
                );
            }
            if needs_progress {
                anchored_row.insert(
                    "progress_in".to_string(),
                    NodeValue::Float(self.layer_progress.unwrap()),
                );
            }
            &anchored_row
        } else {
            &payload.params
        };

        let dst = match ctx.create_texture(size.0.max(1), size.1.max(1)) {
            Ok(t) => t,
            Err(err) => {
                for t in &scratch {
                    ctx.destroy_texture(*t);
                }
                warn(&format!("output texture: {err:#}"));
                return None;
            }
        };

        let result = run_effect(
            &ctx,
            &compiled,
            params,
            &inputs,
            dst,
            size,
            payload.iterations.max(1) as u32,
            if payload.iterative_input.is_empty() {
                None
            } else {
                Some(payload.iterative_input.as_str())
            },
        );
        for t in &scratch {
            ctx.destroy_texture(*t);
        }
        match result {
            Ok(()) => Some(Texture::gpu(
                ctx.clone(),
                dst,
                size.0.max(1),
                size.1.max(1),
                PixelFormat::F32,
            )),
            Err(err) => {
                ctx.destroy_texture(dst);
                warn(&format!("run failed: {err:#}"));
                None
            }
        }
    }
}
impl oak_node::traverser::RenderHooks for RenderEvalHooks {
    fn use_cache(&self) -> bool {
        self.use_cache
    }

    fn is_cancelled(&self) -> bool {
        // TODO(phase-6b): poll the ticket's cancellation flag here so
        // long plugin renders can be interrupted.
        false
    }

    fn resolve(
        &mut self,
        node: oak_node::id::NodeId,
        _row: &NodeValueRow,
        table: &mut NodeValueTable,
    ) {
        let _ = node;
        // One pass over the table: every texture-channel value that boxes
        // a job resolves in place (jobs nested in its inputs first). A
        // job that produces no value leaves its box, so a later row
        // probing the same box still sees the unresolved request.
        let mut in_flight = HashSet::new();
        for (_, value, _) in table.rows_mut() {
            self.resolve_value(value, 0, &mut in_flight);
        }
    }
}

impl Default for RenderEvalHooks {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate the pipeline's canonical frame: F32 RGBA, transparent black,
/// with the given timestamp (the CPU-backend producer for video tickets).
pub fn generate_frame(time: Rational, size: (i32, i32), format: PixelFormat) -> Result<Frame> {
    let (w, h) = size;
    if w <= 0 || h <= 0 {
        return Err(Error::Invalid);
    }
    let mut frame = Frame::new();
    let mut pod = VideoParamsPod::default();
    pod.width = w;
    pod.height = h;
    pod.format = format as i32;
    frame.set_video_params(pod);
    frame.timestamp = time;
    if !frame.allocate() {
        return Err(Error::NoMem);
    }
    Ok(frame)
}

/// The manager-installed ticket producer: render the frame the ticket
/// asks for (F32 pipeline frame). This is the CPU-backend render path.
///
/// M12 P0 routing: a sequence montage (list of clips) is composited
/// topmost-last; a single-footage ticket decodes one stream; otherwise
/// the pipeline frame is generated.
pub fn render_produced_frame(
    time: Rational,
    params: &crate::ticket::VideoTicketParams,
) -> Result<Texture> {
    let (w, h) = params.render_size();
    let format = params.force_format.unwrap_or(PixelFormat::F32);

    if !params.montage.is_empty() {
        let r = render_montage_frame(time, params, (w, h), format);
        return r;
    }
    if let Some((filename, stream_index)) = &params.footage {
        return render_footage_frame(filename, *stream_index, time, (w, h), format);
    }

    // Generated (transparent) frame: prefer a GPU clear so the pipeline
    // stays GPU end to end (M2); fall back to the CPU producer.
    if format == PixelFormat::F32 {
        if let Some(ctx) = oak_core::backend::GpuContext::shared() {
            if let Ok(token) = ctx.create_texture(w, h) {
                if ctx.clear_texture(token).is_ok() {
                    return Ok(Texture::gpu(ctx, token, w, h, PixelFormat::F32));
                }
                ctx.destroy_texture(token);
            }
        }
    }
    let frame = generate_frame(time, (w, h), format)?;
    Ok(Texture::wrap_frame(frame))
}

// ---------------------------------------------------------------------------
// Footage decode (M12 P0): the oakcodec bridge
// ---------------------------------------------------------------------------

/// Process-wide open decoder sessions, keyed by (filename, stream).
/// Sessions are mutex-serialized inside the oakcodec box, so sharing
/// one handle across worker threads is safe. The value carries an LRU
/// tick: the map is capped ([`MAX_CACHED_DECODERS`]) because every
/// session pins an FFmpeg context plus up to two native decoded frames
/// (~50 MB at 4K) — before the cap, scrubbing a footage bin grew the
/// map without bound.
static DECODERS: std::sync::OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<(String, i32), (Arc<dyn oak_codec::decoder::Decoder>, u64)>,
    >,
> = std::sync::OnceLock::new();

/// Cap on cached decoder sessions per process (LRU beyond this). 16
/// covers heavy multi-clip montages without reopen thrash; eviction only
/// drops the map entry — an in-flight render keeps its Arc alive and the
/// session dies with the last reference (Drop releases FFmpeg).
///
/// Eviction is hardware-first: hardware sessions pin GPU memory (each
/// NVDEC decoder holds a surface pool — ~100 MB at 4K), so a full cache
/// with live hardware sessions can exhaust the GPU's video memory and
/// make the NEXT decoder open fail with `cuvidCreateDecoder` OOM (the
/// "4K 切换后大量 CUDA_ERROR_OUT_OF_MEMORY 报错" log flood). Software
/// sessions (system RAM only) are evicted only when nothing else is
/// available.
const MAX_CACHED_DECODERS: usize = 6;

/// LRU tick source for [`DECODERS`].
static DECODER_TICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Count of [`render_footage_frame_inner_opts`] entries, i.e. of real codec work
/// on the footage path (frame-cache hits and decode-service LRU hits do not
/// count). Visible for the decode-service tests, which use it to tell a
/// cached frame from a re-decode.
static DECODE_INVOCATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The number of real footage decodes since the last
/// [`reset_decode_invocations`] (or process start).
pub fn decode_invocations() -> u64 {
    DECODE_INVOCATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset the [`decode_invocations`] counter to zero.
pub fn reset_decode_invocations() {
    DECODE_INVOCATIONS.store(0, std::sync::atomic::Ordering::Relaxed);
}

/// Cap on decoded FRAMES cached per process (release builds of the graph
/// renderer re-decode every frame the pre-render window pulls — each is a
/// seek+decode on the shared decoder session, which serializes the graph
/// path behind the montage baseline. A small frame LRU lets playback pull
/// the forward window from cache instead of thrashing the decoder: the
/// window is sequential, so a short FIFO of recently decoded frames hits
/// on every pre-render restart and on repeat plays).
const MAX_CACHED_FRAMES: usize = 24;

/// Cap on cached PLANAR hardware frames (M5 audit): each planar entry
/// pins a decoder surface through its keep-alive guard, and the driver's
/// surface pool is finite. Planar textures only need to bridge the decode
/// thread → resolve, so they get a much smaller budget than the general
/// frame LRU. Eviction drops only the import; the decoder session cache
/// still holds the raw frame, so a re-request re-imports (no re-decode).
const MAX_CACHED_PLANAR_FRAMES: usize = 4;

/// Decoded-frame LRU key: `(filename, stream, time, w, h)`. The size is
/// part of the key: the same media at a different target resolution is a
/// different frame (an interleaved source-monitor/proxy request must not
/// reuse a wrongly-sized pixel buffer).
type DecodedFrameKey = (String, i32, (i64, i64), i32, i32);

/// Decoded-frame LRU map. Values are decoded textures (CPU frame, or the
/// M5 imported planar texture when the hardware import took the frame)
/// plus their LRU tick. Frame data is the expensive part (a 1080p F32
/// frame ≈ 31 MB); the decoder session cache alone still re-decodes every
/// `render_footage_frame`.
type DecodedFrameCache = std::collections::HashMap<DecodedFrameKey, (Texture, u64)>;

static DECODED_FRAMES: std::sync::OnceLock<std::sync::Mutex<DecodedFrameCache>> =
    std::sync::OnceLock::new();

fn decoded_frames() -> std::sync::MutexGuard<'static, DecodedFrameCache> {
    DECODED_FRAMES
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Insert `texture` under `key` with the general and planar caps applied.
fn insert_cached_frame(cache: &mut DecodedFrameCache, key: DecodedFrameKey, texture: Texture, tick: u64) {
    if cache.contains_key(&key) {
        return;
    }
    while cache.len() >= MAX_CACHED_FRAMES {
        let Some(victim) = cache
            .iter()
            .filter(|(_, (_, t))| *t > 0)
            .min_by_key(|(_, (_, t))| *t)
            .map(|(k, _)| k.clone())
        else {
            break;
        };
        cache.remove(&victim);
    }
    if texture.is_planar() {
        prune_planar_frames(cache, MAX_CACHED_PLANAR_FRAMES.saturating_sub(1));
    }
    cache.insert(key, (texture, tick));
}

/// Evict the least-recently-used planar entries until at most `keep`
/// remain (hardware-surface residency bound, M5 audit).
fn prune_planar_frames(cache: &mut DecodedFrameCache, keep: usize) {
    let mut planar: Vec<(DecodedFrameKey, u64)> = cache
        .iter()
        .filter(|(_, (t, _))| t.is_planar())
        .map(|(k, (_, tick))| (k.clone(), *tick))
        .collect();
    if planar.len() <= keep {
        return;
    }
    planar.sort_by_key(|(_, tick)| *tick);
    for (key, _) in planar.drain(..planar.len() - keep) {
        cache.remove(&key);
    }
}

/// Time-key rational (the frame-LRU's deterministic key half).
fn time_key(t: &Rational) -> (i64, i64) {
    (t.numerator(), t.denominator())
}

fn decoders() -> std::sync::MutexGuard<
    'static,
    std::collections::HashMap<(String, i32), (Arc<dyn oak_codec::decoder::Decoder>, u64)>,
> {
    DECODERS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Pick the victim to evict under the LRU cap: the least-recently-used
/// HARDWARE session when one exists (freeing GPU video memory first,
/// which is the scarce resource), otherwise the least-recently-used
/// session of any kind.
fn eviction_victim(
    cache: &std::collections::HashMap<
        (String, i32),
        (Arc<dyn oak_codec::decoder::Decoder>, u64),
    >,
) -> Option<(String, i32)> {
    cache
        .iter()
        .filter(|(_, (decoder, _))| decoder.hardware_decoding())
        .min_by_key(|(_, (_, t))| *t)
        .map(|(k, _)| k.clone())
        .or_else(|| {
            cache
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
        })
}

/// Open (or reuse) the decoder session for `(filename, stream_index)`.
fn open_decoder(filename: &str, stream_index: i32) -> Result<Arc<dyn oak_codec::decoder::Decoder>> {
    let key = (filename.to_string(), stream_index);
    let tick = DECODER_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if std::env::var_os("OAK_PERF").is_some() {
        eprintln!("[open] {:?} s{} (request)", filename, stream_index);
    }
    {
        let mut cache = decoders();
        if let Some((d, t)) = cache.get_mut(&key) {
            *t = tick;
            if std::env::var_os("OAK_PERF").is_some() {
                eprintln!("[open] {:?} s{} CACHED", filename, stream_index);
            }
            return Ok(d.clone());
        }
    }
    // Hardware-session budget is now handled by the LRU cap below (and the
    // GPU-vram worker-count policy): the "evict the ONLY hardware session
    // before every new open" guard below was eviction-on-every-frame — a
    // live session was dropped before the NEXT request for the same file
    // could hit it, so every frame re-opened the decoder (~0.5 s each) and
    // playback could never keep up (the [open] (request) log flood without
    // a single CACHED hit).
    let decoder: Arc<dyn oak_codec::decoder::Decoder> = Arc::new(FFmpegDecoder::new());
    let stream = CodecStream::with_block(filename.to_string(), stream_index, None);
    decoder
        .open(&stream)
        .map_err(|e| Error::Failed(format!("footage decode open: {e:?}")))?;
    let mut cache = decoders();
    // LRU eviction: hardware-first (free the GPU memory a full cache of
    // hardware sessions pins — the scarcity that breaks the next open);
    // the victim is dropped here, but an in-flight render holds its own
    // Arc — the FFmpeg context (and its GPU surface pool) dies with the
    // last reference.
    while cache.len() >= MAX_CACHED_DECODERS {
        let Some(victim) = eviction_victim(&cache) else {
            break;
        };
        cache.remove(&victim);
    }
    cache.insert(key, (decoder.clone(), tick));
    Ok(decoder)
}

/// Decode the footage frame at `time` and copy/scale it into an
/// oakrender F32 frame of `(w, h)`.
///
/// While a [`crate::pipeline::DecodeService`] is installed (the pipeline
/// backend's decode thread) the decode is a rendezvous with that service,
/// which caches frames and keeps the codec calls off the caller's thread;
/// with no service installed this is the synchronous decode it has always
/// been. Either way the pixels are the same.
pub fn render_footage_frame(
    filename: &str,
    stream_index: i32,
    time: Rational,
    size: (i32, i32),
    format: PixelFormat,
) -> Result<Texture> {
    let started = std::time::Instant::now();
    let decode = |allow_import: bool| -> Result<Texture> {
        match crate::pipeline::decode_service() {
            Some(service) => {
                let request = crate::pipeline::DecodeRequest {
                    filename: filename.to_string(),
                    stream_index,
                    time,
                    size,
                    format,
                    allow_import: true,
                };
                match service.request(request) {
                    Some(result) => result,
                    // The service went away (shutdown) mid-flight: decode here
                    // rather than failing the frame.
                    None => render_footage_frame_inner_opts(
                        filename,
                        stream_index,
                        time,
                        size,
                        format,
                        allow_import,
                    ),
                }
            }
            None => render_footage_frame_inner_opts(
                filename,
                stream_index,
                time,
                size,
                format,
                allow_import,
            ),
        }
    };
    let mut result = decode(true);
    // M5: an imported hardware frame is planar YUV; resolve it to
    // working-space RGBA on the GPU before it leaves the footage path.
    // A resolution failure is a per-frame fallback: re-decode through the
    // CPU scaler with the import disabled (the decoder session cache makes
    // this a transfer, not a second decode).
    if let Ok(texture) = &result {
        if texture.is_planar() {
            result = match resolve_planar_footage(texture) {
                Ok(resolved) => Ok(resolved),
                Err(err) => {
                    eprintln!("planar footage resolve failed, staging fallback: {err:#}");
                    // Bypass the decode service: its cache still holds the
                    // planar entry that just failed to resolve.
                    render_footage_frame_inner_opts(
                        filename,
                        stream_index,
                        time,
                        size,
                        format,
                        false,
                    )
                }
            };
        }
    }
    if std::env::var_os("OAK_PERF").is_some() {
        eprintln!(
            "[decode] {:?} s{} time {}/{} ({:.3}s) size {:?} -> {:.3}s {:?}",
            filename,
            stream_index,
            time.numerator(),
            time.denominator(),
            time.to_f64(),
            size,
            started.elapsed().as_secs_f64(),
            result.is_ok()
        );
    }
    result
}

/// The CPU-staging decode for consumers that composite on the CPU (the
/// montage compositor): same service/FRU semantics as
/// [`render_footage_frame`], but the M5 zero-copy import is disabled and
/// the result is always a CPU frame.
///
/// This is not just an optimization: the montage compositor runs on the
/// CPU, so an imported `Texture::Gpu` would have to be downloaded again —
/// and before this entry point existed it was silently SKIPPED (the M5
/// audit's all-black montage bug). A planar texture must never reach a
/// CPU consumer here.
pub(crate) fn render_footage_frame_staged(
    filename: &str,
    stream_index: i32,
    time: Rational,
    size: (i32, i32),
    format: PixelFormat,
) -> Result<Texture> {
    let result = match crate::pipeline::decode_service() {
        Some(service) => {
            let request = crate::pipeline::DecodeRequest {
                filename: filename.to_string(),
                stream_index,
                time,
                size,
                format,
                allow_import: false,
            };
            match service.request(request) {
                Some(result) => result,
                None => render_footage_frame_inner_opts(
                    filename,
                    stream_index,
                    time,
                    size,
                    format,
                    false,
                ),
            }
        }
        None => render_footage_frame_inner_opts(
            filename,
            stream_index,
            time,
            size,
            format,
            false,
        ),
    };
    // Defensive: a planar texture (a stale service entry from an older
    // key layout) can never satisfy a staging request — re-decode CPU.
    match result {
        Ok(texture) if texture.is_planar() => render_footage_frame_inner_opts(
            filename,
            stream_index,
            time,
            size,
            format,
            false,
        ),
        other => other,
    }
}

/// The synchronous decode behind [`render_footage_frame`] (frame-LRU →
/// decoder session → codec → F32 frame). `pub(crate)` because the decode
/// service runs exactly this on its own thread, with the request's own
/// `allow_import` flag (the montage requests stage on purpose, §M5 audit).
///
/// `allow_import` false forces the CPU staging path: the resolve step's
/// fallback re-decodes with it off, and the montage compositor never
/// wants a GPU frame.
pub(crate) fn render_footage_frame_inner_opts(
    filename: &str,
    stream_index: i32,
    time: Rational,
    size: (i32, i32),
    format: PixelFormat,
    allow_import: bool,
) -> Result<Texture> {
    // (file, stream, time) repeatedly (pre-render restarts, repeated
    // scale-up at the same time, graph + montage interleaving); the
    // decode itself is the expensive part and must not re-run per
    // request. The target size is part of the key (see the cache
    // type's doc).
    let (w, h) = size;
    let cache_size = if w > 0 && h > 0 { (w, h) } else { (-1, -1) };
    let cache_key = (filename.to_string(), stream_index, time_key(&time), cache_size.0, cache_size.1);
    {
        let mut cache = decoded_frames();
        let tick = DECODER_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let texture = cache.get(&cache_key).map(|(f, _)| f.clone());
        if let Some(texture) = texture {
            // A planar entry cannot satisfy a staging request (the resolve
            // step failed and asked for CPU pixels): treat it as a miss and
            // decode through the scaler instead.
            if allow_import || !texture.is_planar() {
                cache.insert(cache_key.clone(), (texture.clone(), tick));
                return Ok(texture);
            }
            cache.remove(&cache_key);
        }
    }
    // Past the frame cache: this call really goes to the codec (the
    // decode-service tests read this counter to prove it).
    DECODE_INVOCATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let decoder = open_decoder(filename, stream_index)?;
    let params = RetrieveVideoParams {
        stream: CodecStream::with_block(filename.to_string(), stream_index, None),
        time,
        length: TimeRange::default(),
        force_range: K_COLOR_RANGE_DEFAULT,
        is_image_sequence: false,
        image_sequence_digits: 0,
        image_sequence_number: 0,
        mode: RenderMode::Offline,
        alpha_is_premultiplied: false,
        // 直接按目标尺寸出帧：swscale 一次完成格式转换 + 缩放，不再
        // 产生全分辨率 F32 中间帧（4K 预览每帧省 ~260MB 瞬时拷贝）。
        target_size: if w > 0 && h > 0 {
            Some((w as u32, h as u32))
        } else {
            None
        },
    };

    // M5: hardware frames are offered to the zero-copy import first; the
    // CPU retrieval below is the per-frame staging fallback (the decoder
    // session cache holds the raw surface, so the fallback never
    // re-decodes).
    if allow_import {
        if let Ok(Some(imported)) = decoder.retrieve_video_frame_gpu(&params) {
            let mut cache = decoded_frames();
            let tick = DECODER_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            insert_cached_frame(&mut cache, cache_key, imported.clone(), tick);
            return Ok(imported);
        }
    }

    let decoded = decoder
        .retrieve_video_frame(&params)
        .map_err(|e| Error::Failed(format!("footage decode at {time:?}: {e:?}")))?;

    let src_w = decoded.width();
    let src_h = decoded.height();
    let src_linesize = decoded.linesize_bytes();
    if src_w <= 0 || src_h <= 0 || src_linesize <= 0 || !decoded.is_allocated() {
        return Err(Error::Failed("footage decode: bad decoded frame".into()));
    }

    // `(0, 0)` means "native size": decode without scaling and produce a
    // frame matching the decoded dimensions (M12: the graph sequence
    // path requests native frames and scales at composite time).
    let (dw, dh) = if w > 0 && h > 0 { (w, h) } else { (src_w, src_h) };
    let mut dst = generate_frame(time, (dw, dh), format)?;
    let dst_linesize = dst.linesize_bytes() as i32;
    let src_data = match decoded.data() {
        Some(d) => d,
        None => return Err(Error::Failed("footage decode: no frame data".into())),
    };

    if src_w == dw && src_h == dh && src_linesize == dst_linesize {
        let bytes = (src_h as usize)
            .checked_mul(src_linesize as usize)
            .ok_or(Error::NoMem)?;
        dst.data[..bytes].copy_from_slice(&src_data[..bytes]);
    } else {
        scale_rgba_f32(
            src_data.as_ptr(),
            src_linesize,
            src_w,
            src_h,
            &mut dst.data,
            dst_linesize,
            dw,
            dh,
        );
    }
    // Input node: source colorspace → the pipeline working space (ACEScg
    // by default; the legacy sRGB working space keeps the pass-through).
    convert_decoded_to_working(&mut dst, &decoded);
    // Memoize the finished working-space frame (LRU-capped).
    let texture = Texture::wrap_frame(dst);
    {
        let mut cache = decoded_frames();
        let tick = DECODER_TICK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        insert_cached_frame(&mut cache, cache_key, texture.clone(), tick);
    }
    Ok(texture)
}

/// Resolve an imported planar hardware frame (M5) to working-space RGBA
/// on the GPU: the YUV→RGB pass (matrix + range from the frame's own
/// colorimetry) followed by the source→working transform. In the legacy
/// sRGB working space the transform is a pass-through (matching the CPU
/// path's `convert_decoded_to_working`); otherwise it is applied as a CPU
/// reference-baked 3D LUT, exactly like the OCIO color transforms.
fn resolve_planar_footage(texture: &Texture) -> Result<Texture> {
    let planar = texture
        .as_planar()
        .ok_or_else(|| Error::Failed("resolve_planar_footage: not planar".into()))?;
    let Some(gpu) = planar
        .ctx
        .as_any()
        .and_then(|a| a.downcast_ref::<oak_core::backend::GpuContext>())
    else {
        return Err(Error::Failed(
            "planar texture belongs to a non-wgpu context".into(),
        ));
    };
    let (w, h) = (planar.width.max(1), planar.height.max(1));
    let dst = gpu
        .create_texture(w, h)
        .map_err(|e| Error::Failed(format!("planar resolve target: {e:?}")))?;
    if let Err(e) = gpu.run_planar_yuv_to_rgb(planar.y, planar.uv, dst, &planar.transform) {
        gpu.destroy_texture(dst);
        return Err(Error::Failed(format!("planar YUV pass: {e:?}")));
    }
    let token = match footage_working_lut(planar.color_primaries, planar.color_trc) {
        Some((key, lut)) => match gpu.apply_color_lut(dst, &key, &lut) {
            Ok(transformed) => {
                gpu.destroy_texture(dst);
                transformed
            }
            Err(e) => {
                eprintln!("footage working-space LUT failed, using source RGB: {e:?}");
                dst
            }
        },
        None => dst,
    };
    Ok(Texture::gpu(planar.ctx.clone(), token, w, h, PixelFormat::F32))
}

/// The source→working-space 3D LUT for a decoded frame (M5 GPU import
/// path): baked from the same CPU reference (`colormath::decode_to_acescg`)
/// the staging path uses, cached per colorimetry. `None` in the legacy
/// sRGB working space (pass-through) or when the LUT cannot be built.
fn footage_working_lut(
    color_primaries: i32,
    color_trc: i32,
) -> Option<(String, std::sync::Arc<oak_core::lut::Lut3d>)> {
    use oak_core::colormath::WorkingColorSpace;
    if oak_core::color::pipeline_working_space() == WorkingColorSpace::SrgbLegacy {
        return None;
    }
    let key = format!("footage/{color_primaries}/{color_trc}");
    type Cache = std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<oak_core::lut::Lut3d>>,
    >;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(lut) = cache.get(&key) {
        return Some((key, lut.clone()));
    }
    let lut = build_footage_working_lut(color_primaries, color_trc)?;
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(key.clone(), lut.clone());
    Some((key, lut))
}

/// Bake the source→ACEScg transform into a 3D LUT over the display
/// domain (the same domain the color-transform LUTs use).
fn build_footage_working_lut(
    color_primaries: i32,
    color_trc: i32,
) -> Option<std::sync::Arc<oak_core::lut::Lut3d>> {
    use oak_core::colormath::{decode_to_acescg, source_primaries_from_av, source_transfer_from_av};
    use oak_core::lut::Lut3d;
    let edge = Lut3d::DISPLAY_EDGE;
    let (lo, hi) = (Lut3d::DISPLAY_LO, Lut3d::DISPLAY_HI);
    let n = (edge as usize).pow(3);
    let mut samples = vec![0.0f32; n * 4];
    let step = |i: usize, axis: usize| -> f32 {
        let t = i as f32 / (edge - 1) as f32;
        lo[axis] + (hi[axis] - lo[axis]) * t
    };
    for b in 0..edge as usize {
        for g in 0..edge as usize {
            for r in 0..edge as usize {
                let idx = ((b * edge as usize + g) * edge as usize + r) * 4;
                samples[idx] = step(r, 0);
                samples[idx + 1] = step(g, 1);
                samples[idx + 2] = step(b, 2);
                samples[idx + 3] = 1.0;
            }
        }
    }
    decode_to_acescg(
        &mut samples,
        source_primaries_from_av(color_primaries),
        source_transfer_from_av(color_trc),
    );
    let mut data = Vec::with_capacity(n * 3);
    for px in samples.chunks_exact(4) {
        data.extend_from_slice(&px[..3]);
    }
    Some(std::sync::Arc::new(Lut3d { edge, lo, hi, data }))
}

/// Convert a decoded footage frame (display-referred RGB in the source's
/// own colorspace) into the pipeline working space, driven by the frame's
/// colorimetry metadata (carried on the codec frame's params). A no-op in
/// the legacy sRGB working space or when the frame has no pixel data.
fn convert_decoded_to_working(dst: &mut Frame, decoded: &oak_codec::frame::Frame) {
    use oak_core::colormath::{source_primaries_from_av, source_transfer_from_av,
                              WorkingColorSpace,
    };
    if oak_core::color::pipeline_working_space() == WorkingColorSpace::SrgbLegacy {
        return;
    }
    // Frames without colorimetry metadata get the generic fallback (sRGB
    // primaries, sRGB transfer) instead of passing through unconverted;
    // the missing tag is warned once per process.
    let (primaries, transfer) = match decoded.params() {
        Some(params) => (
            source_primaries_from_av(params.color_primaries()),
            source_transfer_from_av(params.color_transfer()),
        ),
        None => {
            warn_missing_colorimetry_once();
            (source_primaries_from_av(2), source_transfer_from_av(2))
        }
    };
    let w = dst.width.max(0) as usize;
    let h = dst.height.max(0) as usize;
    if w == 0 || h == 0 {
        return;
    }
    let row_bytes = w * 16; // F32 RGBA
    let linesize = dst.linesize_bytes();
    for y in 0..h {
        let start = y * linesize;
        if start + row_bytes > dst.data.len() {
            break;
        }
        oak_core::colormath::decode_to_acescg_bytes(
            &mut dst.data[start..start + row_bytes],
            w,
            primaries,
            transfer,
        );
    }
}

// ---------------------------------------------------------------------------
// Graph-driven sequence rendering
// ---------------------------------------------------------------------------

/// WGSL fragment for the graph compositor's alpha-over pass (the C++
/// viewer shader is `:/shaders/alphaover.frag`; same premultiplied-over
/// math on raw texture loads). Bindings: 1 = destination (accumulator),
/// 3 = source (the clip frame) — the layout [`GpuContext::compile_shader_pass`]
/// assigns to texture pairs.
const COMP_WGSL: &str = r#"
@group(0) @binding(1) var dst_tex: texture_2d<f32>;
@group(0) @binding(3) var src_tex: texture_2d<f32>;

@fragment
fn main(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = textureDimensions(dst_tex);
    let coord = clamp(vec2<u32>(u32(i32(frag.x)), u32(i32(frag.y))), vec2<u32>(0u, 0u), dims - vec2<u32>(1u, 1u));
    let s = textureLoad(src_tex, coord, 0);
    let d = textureLoad(dst_tex, coord, 0);
    let a = clamp(s.a, 0.0, 1.0);
    // RGB keeps the working-space values unclamped (HDR/WCG can exceed
    // 1.0); only the alpha of the result is clamped to the valid range.
    return vec4<f32>(s.rgb * a + d.rgb * (1.0 - a), clamp(a + d.a * (1.0 - a), 0.0, 1.0));
}
"#;

/// GPU composite of `frames` into one `(w, h)` texture: bottom (last) to
/// top (first), alpha-over into a ping-pong accumulator pair. Frames that
/// do not match `(w, h)` are skipped (the caller scales at decode time;
/// mismatches are defensive).
///
/// GPU→GPU (M2): textures already on the context are used by token; CPU
/// frames are uploaded into scratch textures (counted, and only when the
/// caller has a CPU frame in the stack). Nothing is read back — the
/// result stays on the GPU.
fn composite_tracks_gpu(
	ctx: &std::sync::Arc<oak_core::backend::GpuContext>,
	frames: &[Texture],
	size: (i32, i32),
) -> Result<Texture> {
	let (w, h) = size;
	if w <= 0 || h <= 0 {
		return Err(Error::Invalid);
	}
	let program = ctx.compile_shader_pass("oak/builtin/alpha-over", COMP_WGSL, 2, false, false)?;
	let acc = ctx.create_texture(w, h)?;
	ctx.clear_texture(acc)?;
	let mut scratch: Vec<u64> = Vec::new();
	let mut current = acc;
	let mut owned_current = true;
	let result = (|| -> Result<u64> {
		for frame in frames.iter().rev().filter(|f| f.size() == (w, h)) {
			// Prefer the texture's own context when it is this one; a GPU
			// texture from another context can only be read back.
			let src_token = match frame {
				Texture::Gpu { token, .. } if ctx.has_texture(*token) => *token,
				Texture::Gpu { .. } => {
					let cpu = frame.to_frame()?;
					let t = ctx.create_texture(w, h)?;
					ctx.upload(t, &cpu)?;
					scratch.push(t);
					t
				}
				Texture::Cpu(f) => {
					let t = ctx.create_texture(w, h)?;
					ctx.upload(t, f)?;
					scratch.push(t);
					t
				}
				Texture::Planar(_) => {
					return Err(Error::Failed(
						"unresolved planar texture in composite".into(),
					))
				}
			};
			let out = ctx.create_texture(w, h)?;
			ctx.run_shader_pass(&program, &[], &[current, src_token], out)?;
			if owned_current {
				ctx.destroy_texture(current);
			}
			current = out;
			owned_current = true;
		}
		Ok(current)
	})();
	for t in scratch {
		ctx.destroy_texture(t);
	}
	match result {
		Ok(token) => Ok(Texture::gpu(
			ctx.clone(),
			token,
			w,
			h,
			PixelFormat::F32,
		)),
		Err(err) => {
			if owned_current {
				ctx.destroy_texture(current);
			}
			Err(err)
		}
	}
}

/// GPU failures are remembered: a device whose wgpu pipeline fails
/// validation (e.g. an adapter that advertises ComputePipeline yet lacks
/// the required features) fails EVERY frame otherwise — each attempt
/// recompiles the shader, surfaces a validation error and stalls the
/// playback tick (the "picture barely updates on NVIDIA" report). After
/// one failure the composite stays on the CPU path for the process.
static GPU_COMPOSITE_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Composite of `frames` into one `size` texture. The GPU path is
/// preferred whenever a context is available (M2: the graph stays on the
/// GPU end to end); the CPU path is the no-adapter fallback and the
/// explicit (counted) readback for GPU frames that must go through a CPU
/// consumer.
///
/// Compositing always runs — even a single frame passes through the
/// alpha-over over a transparent accumulator, which is the graph's
/// premultiply step (an adjustment sweep's 0.5-opacity result must become
/// 0.25 after its final composite). Frames arrive topmost first; both
/// paths composite the stack bottom-up (see [`composite_tracks_gpu`]).
fn composite_tracks(frames: Vec<Texture>, size: (i32, i32)) -> Texture {
	let (w, h) = size;
	if w <= 0 || h <= 0 {
		return Texture::dummy();
	}
	if !GPU_COMPOSITE_FAILED.load(std::sync::atomic::Ordering::Relaxed) {
		if let Some(ctx) = oak_core::backend::GpuContext::shared() {
			match composite_tracks_gpu(&ctx, &frames, (w, h)) {
				Ok(texture) => return texture,
				Err(err) => {
					eprintln!(
						"GPU track composite failed, using CPU (and staying there): {err:#}"
					);
					GPU_COMPOSITE_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
				}
			}
		}
	}
	// CPU fallback: read every GPU frame back (the explicit boundary) and
	// composite the CPU stack bottom-up.
	composite_tracks_cpu(&frames, (w, h))
}

/// The CPU half of [`composite_tracks`]: frames arrive topmost first
/// (the render walk inserts each track's frame at the front), so the
/// stack is composited from the bottom (last) up — exactly like
/// [`composite_tracks_gpu`]. This is the path every machine without a
/// working adapter runs, so the layer order must match the GPU pass.
fn composite_tracks_cpu(frames: &[Texture], size: (i32, i32)) -> Texture {
	let (w, h) = size;
	if w <= 0 || h <= 0 {
		return Texture::dummy();
	}
	let Ok(mut acc) = generate_frame(Rational::new(0, 1), (w, h), PixelFormat::F32) else {
		return Texture::dummy();
	};
	let acc_stride = acc.linesize_bytes() as i32;
	for texture in frames.iter().rev() {
		let Ok(frame) = texture.to_frame() else {
			continue;
		};
		if frame.width != w || frame.height != h {
			continue;
		}
		composite_over(
			&mut acc.data,
			acc_stride,
			w,
			h,
			&frame.data,
			frame.linesize_bytes() as i32,
			1.0,
		);
	}
	Texture::wrap_frame(acc)
}

/// One video track's contribution to [`render_graph_frame`], resolved
/// before any evaluation so the project lock is only borrowed immutably
/// (the adjustment sweep needs `&mut` on the very same graph).
enum TrackRenderStep {
    /// The track's enabled clips covering the frame time.
    Clips(Vec<oak_node::id::NodeId>),
    /// The track's enabled transition block covering the frame time: the
    /// two blocks it joins are evaluated at `time` and blended with the
    /// transition's shader. `progress` is the block's position across its
    /// own span, in `0..=1` (`0.0` shows `out_block`, `1.0` shows
    /// `in_block`); `shader` is the style id the block's `type_in` combo
    /// selects.
    Transition {
        block: oak_node::id::NodeId,
        /// The outgoing (previous) block — `None` for a head transition
        /// (no previous clip; fades in from black).
        out_block: Option<oak_node::id::NodeId>,
        /// The incoming (next) block — `None` for a tail transition (no
        /// next clip; fades out to black).
        in_block: Option<oak_node::id::NodeId>,
        progress: f64,
        shader: &'static str,
    },
    /// The track's enabled adjustment block covering the frame time: run
    /// its effect chain over every frame collected below and let the
    /// result replace them (C++ adjustment layers affect everything
    /// underneath). `progress` is the block's position across its own
    /// span, in `0..=1`.
    Adjustment {
        block: oak_node::id::NodeId,
        progress: f64,
    },
}

/// Where `time` sits inside `in_..out`, in `0..=1` (the adjustment
/// layer's `progress_in`). A degenerate span reports 0.
fn layer_progress(in_: Rational, out: Rational, time: Rational) -> f64 {
    let (in_, out, time) = (in_.to_f64(), out.to_f64(), time.to_f64());
    if out > in_ {
        ((time - in_) / (out - in_)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Evaluate one block at `time` and return its texture (GPU when the
/// graph produced one). Every non-texture outcome — the evaluator
/// produced no texture channel or the handle is null — is `Ok(None)` so a
/// caller can fall back instead of failing the whole frame.
fn evaluate_block_frame(
	graph: &oak_node::graph::Graph,
	traverser: &mut oak_node::traverser::Traverser,
	hooks: &mut RenderEvalHooks,
	block: oak_node::id::NodeId,
	time: Rational,
) -> Result<Option<Texture>> {
	let request = oak_node::traverser::EvalRequest::new(block, time);
	let table = traverser.evaluate(graph, &request, hooks).map_err(|e| {
		Error::Failed(format!(
			"graph evaluation of block {block:?} failed: {e:?}"
		))
	})?;
	let Some(NodeValue::Texture(handle)) = table.get(oak_node::value::ValueType::Texture) else {
		return Ok(None);
	};
	if handle.ctx.is_null() {
		return Ok(None);
	}
	let Some(texture) = (unsafe { oak_node::handle::get_checked::<Texture>(handle) }).cloned() else {
		return Ok(None);
	};
	Ok(Some(texture))
}

/// Blend the two sides of a transition block at `time` with the block's
/// style shader: evaluate both sides, box a [`ShaderJobPayload`] carrying
/// the pair and the progress factor, and run it through the render
/// seam's shader-job path (the transition node behavior supplies the
/// fragment source for the shader id). `None` when a side produced no
/// texture or the job could not run (no GPU context, compile failure) —
/// the caller then falls back to the side that natively covers `time`.
fn blend_transition(
    graph: &oak_node::graph::Graph,
    traverser: &mut oak_node::traverser::Traverser,
    hooks: &mut RenderEvalHooks,
    out_block: Option<oak_node::id::NodeId>,
    in_block: Option<oak_node::id::NodeId>,
    time: Rational,
    progress: f64,
    shader: &str,
) -> Result<Option<Texture>> {
    use oak_node::nodes::transitions;
    use oak_node::value::NodeValueRow;

    let from = match out_block {
        Some(block) => evaluate_block_frame(graph, traverser, hooks, block, time)?,
        None => None,
    };
    let to = match in_block {
        Some(block) => evaluate_block_frame(graph, traverser, hooks, block, time)?,
        None => None,
    };
    if from.is_none() && to.is_none() {
        return Ok(None);
    }

    // A single-sided transition blends against transparent black (a head
    // fade-in from nothing, a tail fade-out to nothing) — the same
    // shaders, with one side generated empty. GPU-resident sides blend
    // against a GPU-cleared texture; the CPU fallback generates a frame.
    let size = from
        .as_ref()
        .or(to.as_ref())
        .map(|f| f.size())
        .or(hooks.frame_size)
        .unwrap_or((1, 1));
    let black = |size: (i32, i32)| -> Option<Texture> {
        if let Some(ctx) = oak_core::backend::GpuContext::shared() {
            let token = ctx.create_texture(size.0, size.1).ok()?;
            ctx.clear_texture(token).ok()?;
            return Some(Texture::gpu(
                ctx,
                token,
                size.0,
                size.1,
                PixelFormat::F32,
            ));
        }
        generate_frame(time, size, PixelFormat::F32)
            .ok()
            .map(Texture::wrap_frame)
    };
    let from = from.or_else(|| black(size));
    let to = to.or_else(|| black(size));

    // Both sides present: blend them. GPU sides stay GPU (the shader-job
    // path consumes their tokens directly); CPU sides upload only inside
    // the shader job. The originals stay around for the fallback below.
    let blended = match (from.as_ref(), to.as_ref()) {
        (Some(from), Some(to)) => {
            let payload = ShaderJobPayload {
                node_id: oak_node::id::NodeId::INVALID,
                time,
                iterations: 1,
                type_id: "org.olivevideoeditor.Olive.transition".to_string(),
                shader_id: shader.to_string(),
                effect_input: String::new(),
                params: NodeValueRow::from([
                    (
                        transitions::TEXTURE_INPUT.to_string(),
                        texture_value(from.clone()),
                    ),
                    (
                        transitions::BLEND_INPUT.to_string(),
                        texture_value(to.clone()),
                    ),
                    (
                        transitions::PROGRESS_INPUT.to_string(),
                        NodeValue::Float(progress),
                    ),
                ]),
                iterative_input: String::new(),
            };
            hooks.process_shader_job(&payload)
        }
        _ => None,
    };

    // Fallback: without a blend (a missing side, or a shader job that
    // could not run) show the side that natively covers `time` — the
    // outgoing block before the cut, the incoming one at or after it.
    Ok(blended.or(if progress < 0.5 { from } else { to }))
}

/// Walk an adjustment block's effect chain to its head: the first node
/// whose effect input has no upstream — the node a sweep must feed the
/// composited lower layers into. Returns that node and the input id to
/// connect on it. `None` when there is no chain (a bare adjustment layer
/// contributes nothing and is skipped entirely) or when the walk cannot
/// reach a head (unconnected effect input, or a cycle).
fn adjustment_chain_head(
    graph: &oak_node::graph::Graph,
    block: oak_node::id::NodeId,
) -> Option<(oak_node::id::NodeId, String)> {
    let mut visited = std::collections::HashSet::new();
    let mut node = block;
    loop {
        if !visited.insert(node) {
            return None;
        }
        let entry = graph.get(node)?;
        let input = entry.core.effect_input.clone();
        if input.is_empty() || entry.core.get_input(&input).is_none() {
            return None;
        }
        match graph.connected_output(node, &input, -1) {
            Some(upstream) => node = upstream,
            // The block's own effect input is open: nothing to run.
            None if node == block => return None,
            None => return Some((node, input)),
        }
    }
}

/// Run the effect chain of the adjustment `block` over `below` (the
/// frames of every track underneath it, topmost first): composite them,
/// stand up a temporary texture source holding the result, wire it into
/// the chain head, evaluate the block, and tear the temporary node back
/// down before returning — the graph is part of the live project, so
/// anything inspecting it concurrently must never see a half-wired
/// sweep.
///
/// `Ok(None)` means the boundary changes nothing (no effect chain, or the
/// chain produced no texture); `Ok(Some(texture))` is the chain's output,
/// which replaces the lower layers. The sweep stays on the GPU when the
/// collected layers are GPU textures (M2).
fn flush_adjustment_layer(
    graph: &mut oak_node::graph::Graph,
    traverser: &mut oak_node::traverser::Traverser,
    hooks: &mut RenderEvalHooks,
    block: oak_node::id::NodeId,
    below: &[Texture],
    size: (i32, i32),
    time: Rational,
    progress: f64,
) -> Result<Option<Texture>> {
    let Some((head, head_input)) = adjustment_chain_head(graph, block) else {
        return Ok(None);
    };
    let below = composite_tracks(below.to_vec(), size);
    let (core, behavior) = oak_node::nodes::compositesource::create();
    let source = graph.add_node(core, behavior);
    if let Some(entry) = graph.get_mut(source) {
        entry.core.set_standard_value(
            oak_node::nodes::compositesource::TEXTURE_INPUT,
            -1,
            texture_value(below),
        );
    }
    if let Err(err) = graph.connect(source, head, &head_input, -1) {
        let _ = graph.remove_node(source);
        return Err(Error::Failed(format!(
            "adjustment layer {block:?}: cannot feed {head_input} of {head:?}: {err:?}"
        )));
    }
    hooks.layer_progress = Some(progress);
    let evaluated =
        traverser.evaluate(graph, &oak_node::traverser::EvalRequest::new(block, time), hooks);
    hooks.layer_progress = None;
    let _ = graph.remove_node(source);
    let table = evaluated.map_err(|e| {
        Error::Failed(format!(
            "graph evaluation of adjustment layer {block:?} failed: {e:?}"
        ))
    })?;
    let Some(NodeValue::Texture(handle)) = table.get(oak_node::value::ValueType::Texture) else {
        return Ok(None);
    };
    if handle.ctx.is_null() {
        return Ok(None);
    }
    let Some(texture) = (unsafe { oak_node::handle::get_checked::<Texture>(handle) }).cloned()
    else {
        return Ok(None);
    };
    Ok(Some(texture))
}

/// Render one frame of `viewer` (a sequence) at `time`: evaluate every
/// enabled clip overlapping `time` through the node graph (one traverser
/// pass per clip; the hooks' decoder cache is shared across clips) and
/// composite the resulting frames bottommost-first — in the video track
/// list the LAST track (the highest-numbered one) is the topmost stack
/// element (NLE stacking, matching the timeline UI).
///
/// A track whose enabled adjustment block covers `time` contributes an
/// adjustment sweep (see [`flush_adjustment_layer`]) instead of its clips:
/// everything collected below is composited and pushed through the block's
/// effect chain, and the result replaces the layer stack underneath, so a
/// single block affects every lower track at once.
///
/// A track whose enabled transition block covers `time` contributes one
/// blended frame (see [`blend_transition`]) instead of the clip that
/// covers `time` on its own: both blocks the transition joins are
/// evaluated and mixed by the style shader the block's `type_in` combo
/// selects. An adjustment block still wins over a transition on the same
/// track.
///
/// `size` is the decode target for every clip, so all frames composite
/// without per-frame scaling. Errors: `Invalid` for a non-F32 format or a
/// non-positive size, `NotFound` for a missing viewer or non-sequence.
pub fn render_graph_frame(
    project: &Mutex<oak_node::project::Project>,
    viewer: oak_node::id::NodeId,
    time: Rational,
    size: (i32, i32),
    format: PixelFormat,
) -> Result<Texture> {
    if format != PixelFormat::F32 {
        return Err(Error::Invalid);
    }
    let (w, h) = size;
    if w <= 0 || h <= 0 {
        return Err(Error::Invalid);
    }
    let mut project_guard = project.lock().unwrap();

    // Plan the video tracks bottommost-first, one step per track: the
    // sequence's track lists (video then audio — C++ `Sequence` keeps them
    // in the `k_track_input_format` array order), the video list's tracks
    // in stacking order (the list's last track is the top of the stack;
    // `composite_tracks` walks the frames in reverse and draws the first
    // frame last), then each track's blocks. The plan holds plain ids so
    // the evaluation pass below can borrow the graph mutably for an
    // adjustment sweep's temporary source node.
    let (steps, sequence_size) = {
        let graph = &project_guard.graph;
        let entry = graph.get(viewer).ok_or(Error::NotFound)?;
        let sequence = entry
            .behavior
            .as_any()
            .and_then(|a| a.downcast_ref::<oak_node::sequence::SequenceBehavior>())
            .ok_or(Error::NotFound)?;

        let mut steps: Vec<TrackRenderStep> = Vec::new();
        for tl_id in &sequence.track_lists {
            let Some(tl) = graph.get(*tl_id) else {
                continue;
            };
            let Some(tl) = tl
                .behavior
                .as_any()
                .and_then(|a| a.downcast_ref::<oak_node::track::TrackListBehavior>())
            else {
                continue;
            };
            if tl.kind != oak_node::track::TrackType::Video {
                continue;
            }
            for track_id in &tl.tracks {
                let Some(track) = graph.get(*track_id) else {
                    continue;
                };
                let Some(track) = track
                    .behavior
                    .as_any()
                    .and_then(|a| a.downcast_ref::<oak_node::track::TrackBehavior>())
                else {
                    continue;
                };
                // An enabled adjustment block covering `time` takes over
                // the track: its sweep replaces that track's clip stack.
                let mut step = None;
                for block_id in &track.blocks {
                    let Some(block) = graph.get(*block_id) else {
                        continue;
                    };
                    let Some(adjustment) = block.behavior.as_any().and_then(|a| {
                        a.downcast_ref::<oak_node::block::AdjustmentBlockBehavior>()
                    }) else {
                        continue;
                    };
                    if adjustment.core.enabled
                        && time >= adjustment.core.in_()
                        && time < adjustment.core.out()
                    {
                        step = Some(TrackRenderStep::Adjustment {
                            block: *block_id,
                            progress: layer_progress(
                                adjustment.core.in_(),
                                adjustment.core.out(),
                                time,
                            ),
                        });
                        break;
                    }
                }
                // A transition block covering `time` blends the two blocks
                // it joins. The scan runs before the clip scan so the
                // blend replaces the plain clip read: in the first half of
                // the span the outgoing clip alone covers `time` (the cut
                // is its out-point), in the second the incoming one does,
                // so the clip path would otherwise hard-cut at the cut.
                // A block with an unconnected side falls through to the
                // clip path, which shows whichever clip covers `time`.
                if step.is_none() {
                    for block_id in &track.blocks {
                        let Some(block) = graph.get(*block_id) else {
                            continue;
                        };
                        let Some(transition) = block.behavior.as_any().and_then(|a| {
                            a.downcast_ref::<oak_node::block::TransitionBlockBehavior>()
                        }) else {
                            continue;
                        };
                        if !(transition.core.enabled
                            && time >= transition.core.in_()
                            && time < transition.core.out())
                        {
                            continue;
                        }
                        // A transition needs at least one neighbor: a
                        // junction block wires both, a head/tail
                        // (single-sided) transition wires only its own
                        // clip and fades from/to black.
                        let (out_block, in_block) = (
                            graph.connected_output(
                                *block_id,
                                oak_node::block::transition_input::OUT_BLOCK,
                                -1,
                            ),
                            graph.connected_output(
                                *block_id,
                                oak_node::block::transition_input::IN_BLOCK,
                                -1,
                            ),
                        );
                        if out_block.is_none() && in_block.is_none() {
                            continue;
                        }
                        let style = block
                            .core
                            .value_at_time(
                                oak_node::block::transition_input::TYPE_INPUT,
                                -1,
                                time,
                            )
                            .to_double() as i64;
                        step = Some(TrackRenderStep::Transition {
                            block: *block_id,
                            out_block,
                            in_block,
                            progress: layer_progress(
                                transition.core.in_(),
                                transition.core.out(),
                                time,
                            ),
                            shader: oak_node::nodes::transitions::shader_id_for(style),
                        });
                        break;
                    }
                }
                if step.is_none() {
                    let mut clips: Vec<oak_node::id::NodeId> = Vec::new();
                    for block_id in &track.blocks {
                        let Some(block) = graph.get(*block_id) else {
                            continue;
                        };
                        let Some(clip) = block
                            .behavior
                            .as_any()
                            .and_then(|a| a.downcast_ref::<oak_node::block::ClipBlockBehavior>())
                        else {
                            continue;
                        };
                        if clip.core.enabled && time >= clip.core.in_() && time < clip.core.out() {
                            clips.push(*block_id);
                        }
                    }
                    step = Some(TrackRenderStep::Clips(clips));
                }
                steps.push(step.unwrap());
            }
        }
        let sequence_size = sequence
            .video_params
            .first()
            .map(|p| (p.width.max(1), p.height.max(1)));
        (steps, sequence_size)
    };

    let mut traverser = oak_node::traverser::Traverser::new();
    let mut hooks = RenderEvalHooks::new();
    hooks.frame_size = Some(size);
    // The generators' `resolution_in` anchor (C++ NodeGlobals square
    // resolution): the sequence's native size, so proxy-size playback and
    // full-res paused frames draw generated layers identically.
    hooks.sequence_size = sequence_size;
    let perf = std::env::var_os("OAK_PERF").is_some();
    let mut perf_collect = perf.then(std::time::Instant::now);
    let mut perf_collect_ms = 0.0f64;
    // Topmost frame first (see the track walk above).
    let mut frames: Vec<Texture> = Vec::new();
    let mut perf_clip_hist: Vec<(&'static str, oak_node::id::NodeId, f64)> = Vec::new();
    for step in steps {
        match step {
            TrackRenderStep::Clips(clips) => {
                for clip in clips {
                    let clip_sw = perf.then(std::time::Instant::now);
                    let request = oak_node::traverser::EvalRequest::new(clip, time);
                    let table = traverser
                        .evaluate(&project_guard.graph, &request, &mut hooks)
                        .map_err(|e| {
                            Error::Failed(format!(
                                "graph evaluation of clip {clip:?} failed: {e:?}"
                            ))
                        })?;
                    if perf {
                        perf_clip_hist.push((
                            "clip",
                            clip,
                            clip_sw
                                .map(|t| t.elapsed().as_secs_f64() * 1000.0)
                                .unwrap_or(0.0),
                        ));
                    }
                    if let Some(t0) = &perf_collect {
                        perf_collect_ms += t0.elapsed().as_secs_f64() * 1000.0;
                        perf_collect = Some(std::time::Instant::now());
                    }
                    let Some(NodeValue::Texture(handle)) =
                        table.get(oak_node::value::ValueType::Texture)
                    else {
                        continue;
                    };
                    if handle.ctx.is_null() {
                        continue;
                    }
                    let Some(texture) =
                        (unsafe { oak_node::handle::get_checked::<Texture>(handle) }).cloned()
                    else {
                        continue;
                    };
                    frames.insert(0, texture);
                }
            }
            TrackRenderStep::Transition {
                block,
                out_block,
                in_block,
                progress,
                shader,
            } => {
                let transition_sw = perf.then(std::time::Instant::now);
                let blended = blend_transition(
                    &project_guard.graph,
                    &mut traverser,
                    &mut hooks,
                    out_block,
                    in_block,
                    time,
                    progress,
                    shader,
                )?;
                if perf {
                    perf_clip_hist.push((
                        "transition",
                        block,
                        transition_sw
                            .map(|t| t.elapsed().as_secs_f64() * 1000.0)
                            .unwrap_or(0.0),
                    ));
                }
                if let Some(t0) = &perf_collect {
                    perf_collect_ms += t0.elapsed().as_secs_f64() * 1000.0;
                    perf_collect = Some(std::time::Instant::now());
                }
                if let Some(frame) = blended {
                    frames.insert(0, frame);
                }
            }
            TrackRenderStep::Adjustment { block, progress } => {
                let sweep_sw = perf.then(std::time::Instant::now);
                let swept = flush_adjustment_layer(
                    &mut project_guard.graph,
                    &mut traverser,
                    &mut hooks,
                    block,
                    &frames,
                    size,
                    time,
                    progress,
                )?;
                if perf {
                    perf_clip_hist.push((
                        "adjustment",
                        block,
                        sweep_sw
                            .map(|t| t.elapsed().as_secs_f64() * 1000.0)
                            .unwrap_or(0.0),
                    ));
                }
                if let Some(t0) = &perf_collect {
                    perf_collect_ms += t0.elapsed().as_secs_f64() * 1000.0;
                    perf_collect = Some(std::time::Instant::now());
                }
                if let Some(frame) = swept {
                    frames = vec![frame];
                }
            }
        }
    }

    let composite_started = perf.then(std::time::Instant::now);
    let mut frame = composite_tracks(frames, size);
    if let Texture::Cpu(cpu) = &mut frame {
        // The GPU path carries no timestamp (there is nowhere to put it);
        // the 10-bit present path uses the ticket's time, not this field.
        cpu.timestamp = time;
    }
    if perf {
        let composite_ms = composite_started
            .map(|t| t.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        for (kind, node, ms) in &perf_clip_hist {
            eprintln!("[perf]   {kind} {node:?} evaluate {ms:.1}ms");
        }
        eprintln!(
            "[perf] graph frame time {time:?} size {size:?}: evaluate {perf_collect_ms:.1}ms composite {composite_ms:.1}ms total {}ms",
            perf_collect_ms + composite_ms
        );
    }
    Ok(frame)
}

/// Render the audio montage over `params.range` (M12 P1): every clip
/// overlapping the range is decoded (interleaved f32 at the output rate
/// and layout) and mixed with its gain; uncovered parts stay silent. The
/// mixed output is clamped to [-1, 1].
pub fn render_audio_samples(
    params: &crate::ticket::AudioTicketParams,
) -> Result<crate::ticket::TicketPayload> {
    let (rate, layout, channels, total_frames) = audio_layout(params)?;
    let mut acc = vec![0.0f32; total_frames.saturating_mul(channels as usize)];
    mix_audio_montage(params, rate, channels, total_frames, &mut acc)?;
    Ok(crate::ticket::TicketPayload::Audio(crate::ticket::AudioSamples {
        samples: acc,
        sample_rate: rate,
        channel_layout: layout,
        channel_count: channels,
    }))
}

/// The output layout an audio render produces: `(sample_rate,
/// channel_layout, channel_count, total_sample_frames)`.
fn audio_layout(params: &crate::ticket::AudioTicketParams) -> Result<(i32, u64, i32, usize)> {
    let rate = params.sample_rate.max(1);
    let channels = params.channel_layout.count_ones().max(1) as i32;
    let duration = params.range.out() - params.range.in_();
    let seconds = if duration.denominator() == 0 {
        0.0
    } else {
        duration.numerator() as f64 / duration.denominator() as f64
    };
    if seconds <= 0.0 || seconds > 3600.0 {
        return Err(Error::Invalid);
    }

    // Anchor each chunk to the absolute sample grid (`round(out·rate) -
    // round(in·rate)`) instead of rounding the duration: at fractional
    // frame rates (29.97 fps → 1601.6 samples/frame) a duration-round
    // would emit 1602 samples for every chunk and accumulate ~12 extra
    // samples per second, slowly desyncing audio from video. Per-chunk
    // anchoring keeps the total exact and matches the decode side
    // (`FFmpegDecoder::retrieve_audio_to` fills `round(out·rate) -
    // round(in·rate)` samples).
    let total_frames = ((params.range.out().to_f64() * rate as f64).round()
        - (params.range.in_().to_f64() * rate as f64).round())
        .max(0.0) as usize;
    Ok((rate, params.channel_layout, channels, total_frames))
}

/// The byte length (interleaved f32, little-endian) an audio render of
/// `params` writes into a shm slot — the worker's slot-geometry check
/// (M15 S3). Mirrors [`render_audio_samples_into`]'s layout math.
pub fn audio_samples_byte_len(params: &crate::ticket::AudioTicketParams) -> Result<usize> {
    let (_rate, _layout, channels, total_frames) = audio_layout(params)?;
    Ok(total_frames
        .saturating_mul(channels as usize)
        .saturating_mul(4))
}

/// Mix the audio montage into `acc` (`total_frames * channels` samples,
/// zero-initialized by the caller). Shared by the heap
/// [`render_audio_samples`] and the shm-slot [`render_audio_samples_into`]
/// paths so the decode/mix logic exists once.
fn mix_audio_montage(
    params: &crate::ticket::AudioTicketParams,
    rate: i32,
    channels: i32,
    total_frames: usize,
    acc: &mut [f32],
) -> Result<()> {
    for clip in &params.montage {
        // Overlap of the clip with the requested range.
        let in_time = params.range.in_().max(clip.in_time);
        let out_time = params.range.out().min(clip.out_time);
        if out_time <= in_time {
            continue;
        }
        // Round to the absolute sample grid like the decode side (which
        // anchors at `round(media_start·rate)`): truncation would shift a
        // clip's mix by up to one sample per chunk.
        let start_frame = ((in_time - params.range.in_()).to_f64() * rate as f64).round() as usize;
        let end_frame = ((out_time - params.range.in_()).to_f64() * rate as f64).round() as usize;
        if start_frame >= total_frames {
            continue;
        }
        let frames = (end_frame - start_frame).min(total_frames - start_frame);
        if frames == 0 {
            continue;
        }

        // Media time of the overlap start; the media-out is
        // media_start + (overlap duration).
        let media_start = clip.media_in + (in_time - clip.in_time);
        let media_end = media_start + (out_time - in_time);
        let mut buf = vec![0.0f32; frames * channels as usize];
        let decoder = open_decoder(&clip.filename, clip.stream_index)?;
        let range = TimeRange::new(media_start, media_end);
        let status = decoder
            .retrieve_audio(&mut buf, &range, rate, params.channel_layout)
            .map_err(|e| Error::Failed(format!("footage audio decode: {e:?}")))?;
        let written = match status {
            RetrieveAudioStatus::Success => frames,
            _ => 0,
        };
        // Mix into the accumulator (per-channel gain).
        for i in 0..written * channels as usize {
            acc[start_frame * channels as usize + i] += buf[i] * clip.gain;
        }
    }
    // Clamp to [-1, 1]: overlapping clips sum linearly and can exceed
    // full scale, and the playback sink (cpal) forwards samples without
    // any clamping of its own.
    for sample in acc.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
    Ok(())
}

/// Render the audio montage over `params.range` directly into `dst` as
/// little-endian f32 bytes (M15 S3 worker seam): the render worker passes
/// a shared-memory slot slice as `dst`, so the samples land in the slot
/// with no staging allocation. `dst.len()` must hold
/// `frame_count * channels * 4` bytes.
pub fn render_audio_samples_into(
    params: &crate::ticket::AudioTicketParams,
    dst: &mut [u8],
) -> Result<()> {
    let (rate, _layout, channels, total_frames) = audio_layout(params)?;
    let need = total_frames
        .saturating_mul(channels as usize)
        .saturating_mul(4);
    if dst.len() < need {
        return Err(Error::NoMem);
    }
    let mut acc = vec![0.0f32; total_frames.saturating_mul(channels as usize)];
    mix_audio_montage(params, rate, channels, total_frames, &mut acc)?;
    // Interleaved f32 -> little-endian bytes in the slot.
    for (out, sample) in dst[..need].chunks_exact_mut(4).zip(&acc) {
        out.copy_from_slice(&sample.to_le_bytes());
    }
    Ok(())
}

/// Bilinear scale an F32-RGBA image (row-major with per-row strides).
fn scale_rgba_f32(
    src: *const u8,
    src_stride: i32,
    src_w: i32,
    src_h: i32,
    dst: &mut [u8],
    dst_stride: i32,
    dst_w: i32,
    dst_h: i32,
) {
    if src_w <= 0 || src_h <= 0 || dst_w <= 0 || dst_h <= 0 {
        return;
    }
    // 1:1 copy (no scaling): the caller already handled stride equality;
    // here we handle the general case with a fast path for integer 1:1.
    let sample = |x: f64, y: f64| -> [f32; 4] {
        let x0 = x.floor() as i32;
        let y0 = y.floor() as i32;
        let fx = (x - x0 as f64) as f32;
        let fy = (y - y0 as f64) as f32;
        let x1 = (x0 + 1).clamp(0, src_w - 1);
        let y1 = (y0 + 1).clamp(0, src_h - 1);
        let x0 = x0.clamp(0, src_w - 1);
        let y0 = y0.clamp(0, src_h - 1);
        let px = |xx: i32, yy: i32| -> [f32; 4] {
            let off = (yy as usize) * (src_stride as usize) + (xx as usize) * 16;
            // SAFETY: coordinates are clamped to the source size.
            let b = unsafe { std::slice::from_raw_parts(src.add(off), 16) };
            let f = |i: usize| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
            [f(0), f(1), f(2), f(3)]
        };
        let c00 = px(x0, y0);
        let c10 = px(x1, y0);
        let c01 = px(x0, y1);
        let c11 = px(x1, y1);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let mut out = [0f32; 4];
        for i in 0..4 {
            let top = lerp(c00[i], c10[i], fx);
            let bottom = lerp(c01[i], c11[i], fx);
            out[i] = lerp(top, bottom, fy);
        }
        out
    };
    for y in 0..dst_h {
        let sy = (y as f64 + 0.5) * src_h as f64 / dst_h as f64 - 0.5;
        let sy = sy.max(0.0);
        for x in 0..dst_w {
            let sx = (x as f64 + 0.5) * src_w as f64 / dst_w as f64 - 0.5;
            let sx = sx.max(0.0);
            let px = sample(sx, sy);
            let off = (y as usize) * (dst_stride as usize) + (x as usize) * 16;
            // RGB is not clamped: bilinear lerp is a convex combination,
            // so values cannot overshoot the source range, and HDR/WCG
            // working-space pixels may legitimately exceed 1.0. Only the
            // alpha channel is clamped to its valid range.
            for i in 0..4 {
                let v = if i == 3 { px[i].clamp(0.0, 1.0) } else { px[i] };
                dst[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

/// Composite the montage at `time`: decode each covering clip and
/// alpha-composite topmost-last (NLE track order: the highest-numbered
/// track is topmost).
fn render_montage_frame(
    time: Rational,
    params: &crate::ticket::VideoTicketParams,
    size: (i32, i32),
    format: PixelFormat,
) -> Result<Texture> {
    let mut acc = generate_frame(time, size, format)?;
    let stride = acc.linesize_bytes();
    let acc_data = &mut acc.data;
    render_montage_frame_into(time, params, size, acc_data, stride as i32)?;
    Ok(Texture::wrap_frame(acc))
}

/// Composite the montage at `time` directly into `dst` (F32 RGBA rows of
/// `dst_stride` bytes) — the M15 worker seam: the render worker passes a
/// shared-memory slot slice as `dst`, so the composited frame lands in
/// the slot with no staging copy. `dst` is zeroed first (transparent
/// black base).
///
/// Adjustment layers (`params.adjustments`, M14 W3) sit between the
/// clips: a span's effect stack runs over the frame accumulated so far
/// once `span.track_index` clips below it have been composited — the
/// montage twin of the graph path's `flush_adjustment_layer`, which
/// grades everything under the layer and leaves the tracks above
/// compositing over that result. Spans arrive bottom-up (non-decreasing
/// boundary); the loop consumes them in list order.
pub fn render_montage_frame_into(
    time: Rational,
    params: &crate::ticket::VideoTicketParams,
    size: (i32, i32),
    dst: &mut [u8],
    dst_stride: i32,
) -> Result<()> {
    let (w, h) = size;
    let need = (h as usize).saturating_mul(dst_stride as usize);
    if w <= 0 || h <= 0 || dst.len() < need {
        return Err(Error::Invalid);
    }
    // Transparent-black base.
    dst[..need].fill(0);
    let mut spans = params.adjustments.iter().peekable();
    // Decode from the bottom clip first, composite topmost-last.
    for (index, clip) in params.montage.iter().enumerate() {
        // Every layer whose boundary is reached by the clips composited
        // so far grades the accumulated frame before this clip lands.
        while spans.peek().is_some_and(|span| span.track_index <= index) {
            let span = spans.next().expect("peeked");
            apply_adjustment_span(dst, dst_stride, w, h, span, time);
        }
        if time < clip.in_time || time >= clip.out_time {
            continue;
        }
        let media_time = clip.media_in + (time - clip.in_time);
        // CPU compositor: stage on purpose (never an imported GPU frame).
        let decoded = render_footage_frame_staged(
            &clip.filename,
            clip.stream_index,
            media_time,
            (w, h),
            PixelFormat::F32,
        )?;
        // The clip's effect stack runs between decode and compositing
        // (C++ semantics: the clip texture passes through the chain
        // bottom-up, the chain top feeds the track composite).
        let effected = apply_clip_effects(decoded, clip, time);
        // The compositor is CPU-side. The decode is staged above, but a
        // clip effect may still hand back a GPU texture: read it back
        // (an explicit CPU boundary) instead of silently dropping the
        // clip (M5 audit: that skip produced all-black sequences).
        let downloaded: Frame;
        let (src_data, src_stride) = match &effected {
            Texture::Cpu(src) => (&src.data, src.linesize_bytes() as i32),
            Texture::Gpu { .. } => {
                downloaded = effected.to_frame().map_err(|e| {
                    Error::Failed(format!("montage GPU readback failed: {e:?}"))
                })?;
                (&downloaded.data, downloaded.linesize_bytes() as i32)
            }
            Texture::Planar(_) => {
                return Err(Error::Failed(
                    "unresolved planar texture reached the montage compositor".into(),
                ))
            }
        };
        composite_over(dst, dst_stride, w, h, src_data, src_stride, clip.gain);
    }
    // Layers above every clip (their boundary is the montage end): they
    // grade the final composite.
    for span in spans {
        apply_adjustment_span(dst, dst_stride, w, h, span, time);
    }
    Ok(())
}

/// Apply an adjustment layer's effect stack to the frame accumulated so
/// far — the montage twin of the graph path's `flush_adjustment_layer`.
/// The stack runs source-first over `dst` before the clips above the
/// layer are composited, so the layer grades exactly the picture
/// underneath it (C++: the adjustment node's `texture_input` chain
/// output passes through its effect chain, and that output is what the
/// tracks above composite over; a bare adjustment layer passes the
/// frame through unchanged).
///
/// Known limitation (M14 W3 — the same one the clip stacks carry): the
/// montage evaluator only covers the built-in Opacity effect and OFX
/// plugins; other built-ins (color management, transforms, generated
/// textures…) log a warning once and pass the frame through. Graph mode
/// (`render_graph_frame`, M14 W2) evaluates the full built-in set, so a
/// worker holding a matching project snapshot stays the accurate
/// preview and this path is the montage fallback.
fn apply_adjustment_span(
    dst: &mut [u8],
    dst_stride: i32,
    w: i32,
    h: i32,
    span: &crate::ticket::AdjustmentSpan,
    time: Rational,
) {
    // Spans are baked for the ticket's own time; one that does not cover
    // it (a stale or foreign list) is inert, matching the graph path's
    // range test.
    if time < span.in_time || time >= span.out_time {
        return;
    }
    // Fast path: an all-Opacity stack (the common case) scales the
    // accumulated frame in place, no staging copy — opacity is a pure
    // per-channel multiply, alpha included (C++ `:/shaders/opacity.frag`).
    let mut factors = Vec::new();
    let mut opacity_only = true;
    for effect in span.effects.iter().filter(|e| e.enabled) {
        match opacity_factor(effect) {
            Some(factor) => factors.push(factor),
            // Unity is a pass-through; any other type needs the staged
            // path below.
            None if effect.type_id == OPACITY_EFFECT_TYPE_ID => {}
            None => {
                opacity_only = false;
                break;
            }
        }
    }
    if opacity_only {
        for factor in factors {
            scale_channels_in_place(dst, dst_stride as usize, w, h, factor);
        }
        return;
    }
    // General path: the effect evaluator takes and returns an owned
    // texture, so stage the accumulated frame into one and copy the
    // result back.
    let Ok(mut frame) = generate_frame(time, (w, h), PixelFormat::F32) else {
        return;
    };
    let frame_stride = frame.linesize_bytes();
    if !copy_rows(
        &mut frame.data,
        frame_stride,
        dst,
        dst_stride as usize,
        w,
        h,
    ) {
        return;
    }
    let tex = apply_effect_list(Texture::wrap_frame(frame), &span.effects, time);
    if let Texture::Cpu(out) = &tex {
        copy_rows(dst, dst_stride as usize, &out.data, out.linesize_bytes(), w, h);
    }
}

/// Copy `h` rows of `w * 16` bytes between two F32 RGBA buffers with
/// different strides (the montage pipeline writes packed frames, slots
/// carry their own stride). False — nothing copied — when either buffer
/// is too small for the requested geometry.
fn copy_rows(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    w: i32,
    h: i32,
) -> bool {
    if w <= 0 || h <= 0 {
        return false;
    }
    let row_bytes = (w as usize) * 16;
    let rows = h as usize;
    let span = |stride: usize| (rows - 1) * stride + row_bytes;
    if src.len() < span(src_stride) || dst.len() < span(dst_stride) {
        return false;
    }
    for y in 0..rows {
        let s = y * src_stride;
        let d = y * dst_stride;
        dst[d..d + row_bytes].copy_from_slice(&src[s..s + row_bytes]);
    }
    true
}

/// `src` over `dst` (premultiplied-ish alpha compositing; F32 RGBA).
/// `gain` scales the source RGB (audio-style volume applied to video
/// transparency is ignored here; gain scales color). Exposed for the M15
/// render worker, which composites montage frames directly into
/// shared-memory slots.
pub fn composite_over(
    dst: &mut [u8],
    dst_stride: i32,
    w: i32,
    h: i32,
    src: &[u8],
    src_stride: i32,
    gain: f32,
) {
    let read = |buf: &[u8], stride: i32, x: i32, y: i32| -> [f32; 4] {
        let off = (y as usize) * (stride as usize) + (x as usize) * 16;
        let mut out = [0f32; 4];
        for i in 0..4 {
            out[i] = f32::from_le_bytes(buf[off + i * 4..off + i * 4 + 4].try_into().unwrap());
        }
        out
    };
    for y in 0..h {
        for x in 0..w {
            let s = read(src, src_stride, x, y);
            let d = read(dst, dst_stride, x, y);
            let a = (s[3] * gain).clamp(0.0, 1.0);
            let out = [
                (s[0] * gain) * a + d[0] * (1.0 - a),
                (s[1] * gain) * a + d[1] * (1.0 - a),
                (s[2] * gain) * a + d[2] * (1.0 - a),
                a + d[3] * (1.0 - a),
            ];
            let off = (y as usize) * (dst_stride as usize) + (x as usize) * 16;
            // RGB keeps the working-space values unclamped (HDR/WCG can
            // exceed 1.0); only alpha is clamped to its valid range.
            for i in 0..4 {
                let v = if i == 3 { out[i].clamp(0.0, 1.0) } else { out[i] };
                dst[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Montage clip effect stacks
// ---------------------------------------------------------------------------

/// The built-in Opacity effect's type id (oaknode `OpacityEffect`) — the
/// one built-in video effect with a CPU evaluator on the montage path.
const OPACITY_EFFECT_TYPE_ID: &str = "org.olivevideoeditor.Olive.opacity";

/// The Opacity effect's value input id (oaknode `opacity_in`).
const OPACITY_VALUE_INPUT: &str = "opacity_in";

/// The effect type ids the montage path already warned about (one log
/// line per type per process instead of one per frame).
fn unsupported_warned() -> std::sync::MutexGuard<'static, std::collections::HashSet<String>> {
    static WARNED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    WARNED
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Build (and cache) the 3D LUT for an OCIO color processor (M2): the
/// CPU reference (`convert_f32_rgba`) fills the grid, and the per-pixel
/// transform then runs on the GPU via `GpuContext::apply_color_lut` —
/// color management is never skipped on a GPU texture. Cached by the
/// processor's OCIO cache id.
fn color_transform_lut(
    processor: &oak_core::color::ColorProcessor,
) -> Option<std::sync::Arc<oak_core::lut::Lut3d>> {
    type Cache = std::sync::Mutex<
        std::collections::HashMap<String, std::sync::Arc<oak_core::lut::Lut3d>>,
    >;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let key = processor.cache_id();
    let mut cache = CACHE
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(lut) = cache.get(&key) {
        return Some(lut.clone());
    }
    let lut = build_color_transform_lut(processor)?;
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(key, lut.clone());
    Some(lut)
}

/// Bake a processor into a 3D LUT over the display domain (scene-linear
/// working values; the same range the presentation LUT covers).
fn build_color_transform_lut(
    processor: &oak_core::color::ColorProcessor,
) -> Option<std::sync::Arc<oak_core::lut::Lut3d>> {
    use oak_core::lut::Lut3d;
    let edge = Lut3d::DISPLAY_EDGE;
    let (lo, hi) = (Lut3d::DISPLAY_LO, Lut3d::DISPLAY_HI);
    let n = (edge as usize).pow(3);
    let mut samples = vec![0.0f32; n * 4];
    let step = |i: usize, axis: usize| -> f32 {
        let t = i as f32 / (edge - 1) as f32;
        lo[axis] + (hi[axis] - lo[axis]) * t
    };
    for b in 0..edge as usize {
        for g in 0..edge as usize {
            for r in 0..edge as usize {
                let idx = ((b * edge as usize + g) * edge as usize + r) * 4;
                samples[idx] = step(r, 0);
                samples[idx + 1] = step(g, 1);
                samples[idx + 2] = step(b, 2);
                samples[idx + 3] = 1.0;
            }
        }
    }
    processor.convert_f32_rgba(&mut samples, n as i64).ok()?;
    let mut data = Vec::with_capacity(n * 3);
    for px in samples.chunks_exact(4) {
        data.extend_from_slice(&px[..3]);
    }
    Some(std::sync::Arc::new(Lut3d { edge, lo, hi, data }))
}

/// Log an unsupported-effect passthrough once per type id.
fn warn_unsupported_once(type_id: &str, reason: &str) {
    if unsupported_warned().insert(type_id.to_string()) {
        eprintln!("montage effect \"{type_id}\" passes through unchanged: {reason}");
    }
}

/// Warn once (per process) when a decoded frame carries no colorimetry
/// metadata and falls back to the generic sRGB assumptions.
fn warn_missing_colorimetry_once() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        eprintln!("decoded frame has no colorimetry metadata; assuming sRGB");
    });
}

/// Run a clip's effect stack over its decoded frame (source-first order;
/// disabled effects are bypassed — the C++ traverser's bypass pushes the
/// effect input through unchanged). Effects the montage path cannot
/// evaluate log a warning once and pass the frame through.
fn apply_clip_effects(
    src: Texture,
    clip: &crate::ticket::MontageClip,
    time: Rational,
) -> Texture {
    apply_effect_list(src, &clip.effects, time)
}

/// Run an effect stack (source-first) over `src`; the clip stacks and the
/// adjustment-layer stacks share this evaluator.
fn apply_effect_list(
    src: Texture,
    effects: &[crate::ticket::MontageEffect],
    time: Rational,
) -> Texture {
    let mut tex = src;
    for effect in effects {
        if !effect.enabled {
            continue;
        }
        tex = apply_montage_effect(tex, effect, time);
    }
    tex
}

/// The factor an Opacity effect would scale its input by: `None` when
/// the effect is not the built-in Opacity or its factor is unity (C++
/// `qFuzzyCompare(opacity, 1.0)`, a pass-through).
fn opacity_factor(effect: &crate::ticket::MontageEffect) -> Option<f32> {
    if effect.type_id != OPACITY_EFFECT_TYPE_ID {
        return None;
    }
    let factor = effect
        .params
        .iter()
        .find(|(id, _)| id == OPACITY_VALUE_INPUT)
        .map(|(_, v)| v.to_double())
        .unwrap_or(1.0);
    if (factor - 1.0).abs() * 1e12 <= factor.abs().min(1.0) {
        return None;
    }
    Some(factor as f32)
}

/// Scale every F32 channel (alpha included) of an F32 RGBA frame by
/// `factor`, in place — the Opacity shader's `frag_color * opacity_in`.
fn scale_channels_in_place(
    data: &mut [u8],
    stride: usize,
    width: i32,
    height: i32,
    factor: f32,
) {
    if stride == 0 || width <= 0 || height <= 0 {
        return;
    }
    let row_bytes = (width as usize) * 16;
    for row in data.chunks_exact_mut(stride).take(height as usize) {
        let row_end = row_bytes.min(row.len());
        for px in row[..row_end].chunks_exact_mut(16) {
            for c in px.chunks_exact_mut(4) {
                let v = f32::from_le_bytes(c.try_into().unwrap());
                c.copy_from_slice(&(v * factor).to_le_bytes());
            }
        }
    }
}

/// Apply one effect to `src` (an F32 RGBA CPU frame of the montage
/// pipeline). `time` is the sequence time the frame is rendered at (the
/// C++ node evaluation time).
fn apply_montage_effect(
    src: Texture,
    effect: &crate::ticket::MontageEffect,
    time: Rational,
) -> Texture {
    // Built-in Opacity: multiply every channel by the opacity factor
    // (C++ `:/shaders/opacity.frag`: `frag_color = texture(tex_in, …) *
    // opacity_in` — the shader scales the whole vec4, alpha included).
    if effect.type_id == OPACITY_EFFECT_TYPE_ID {
        // Unity is a pass-through (C++ `qFuzzyCompare(opacity, 1.0)`).
        let Some(factor) = opacity_factor(effect) else {
            return src;
        };
        // Texture implements Drop, so scale in place through a mutable
        // borrow instead of moving the frame out.
        let mut tex = src;
        match &mut tex {
            Texture::Cpu(frame) => {
                let stride = frame.linesize_bytes();
                scale_channels_in_place(&mut frame.data, stride, frame.width, frame.height, factor);
            }
            _ => {
                warn_unsupported_once(
                    &effect.type_id,
                    "opacity on a non-CPU texture is not supported by the montage path",
                );
            }
        }
        return tex;
    }

    // Everything else: an OFX plugin effect. The montage carries the
    // plugin identifier; the rendering process resolves it to a live
    // instance through the oakplugin-installed factory (lazily created
    // and cached per identifier), then dispatches through the plugin
    // executor exactly like the graph path's plugin jobs. When the single
    // OFX host client is installed the local instance is not needed: the
    // host resolves the identifier itself.
    let instance = if crate::ofxhost::client().is_some() {
        0
    } else {
        let Some(factory) = plugin_instance_factory() else {
            warn_unsupported_once(
                &effect.type_id,
                "no plugin instance factory installed (oakplugin init missing in this process)",
            );
            return src;
        };
        let Some(instance) = factory(&effect.type_id) else {
            warn_unsupported_once(
                &effect.type_id,
                "no evaluator: unknown built-in effect or OFX plugin unavailable in this process",
            );
            return src;
        };
        instance
    };
    let spec = JobSpec::Plugin {
        instance,
        type_id: effect.type_id.clone(),
        time: time.to_f64(),
        effect_input_id: effect.effect_input_id.clone(),
        inputs: Vec::new(),
        values: effect.params.clone(),
    };
    let size = src.size();
    match RenderEvalHooks::new().process_plugin_job(src, &spec) {
        Ok(texture) => texture,
        Err(err) => {
            // Unreachable for a Plugin spec (the executor failure path
            // yields a purple frame); stay loud rather than silent.
            eprintln!("montage plugin job failed to dispatch: {err:#}");
            purple_frame(time, size)
        }
    }
}

/// The crate-level test lock serializing every test that reads or writes
/// the process-global pipeline color settings
/// ([`oak_core::color::set_pipeline_color_settings`]). The `pipeline` decode
/// tests pin the legacy working space for the whole decoded-pattern section
/// while the tests below temporarily switch to ACEScg; both modules run in
/// the same test binary, so one shared lock is required.
#[cfg(test)]
pub(crate) fn working_space_test_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_frame_is_f32_transparent_black() {
        let f = generate_frame(Rational::new(5, 1), (64, 48), PixelFormat::F32).unwrap();
        assert_eq!(f.width, 64);
        assert_eq!(f.height, 48);
        assert_eq!(f.format, PixelFormat::F32);
        assert_eq!(f.timestamp, Rational::new(5, 1));
        assert!(f.data.iter().all(|&b| b == 0), "transparent black");
        assert_eq!(f.data.len(), 64 * 48 * 4 * 4);
    }

    #[test]
    fn generated_frame_rejects_bad_size() {
        assert!(generate_frame(Rational::new(0, 1), (0, 10), PixelFormat::F32).is_err());
        assert!(generate_frame(Rational::new(0, 1), (-1, 10), PixelFormat::F32).is_err());
    }

    #[test]
    fn produced_frame_honors_ticket_params() {
        let params = crate::ticket::VideoTicketParams {
            viewer: 1,
            project: String::new(),
            time: Rational::new(2, 1),
            force_size: Some((16, 9)),
            force_format: Some(PixelFormat::F32),
            cache: None,
            cache_dir: None,
            cache_id: None,
            cache_timebase: None,
            footage: None,
            montage: Vec::new(),
        	adjustments: Vec::new(),
        };
        let tex = render_produced_frame(params.time, &params).unwrap();
        assert_eq!(tex.size(), (16, 9));
        assert_eq!(tex.format(), PixelFormat::F32);
    }

    /// A temporary disk frame-cache path for the cache-job tests.
    fn cache_job_temp_path(tag: &str) -> String {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "oakrender_cache_{}_{n}_{tag}.bin",
                std::process::id()
            ))
            .to_string_lossy()
            .into_owned()
    }

    /// The resolved texture in the table's texture channel; panics when the
    /// value is still a job box (resolution did not run).
    fn resolved_texture(table: &NodeValueTable) -> Texture {
        let Some(NodeValue::Texture(handle)) = table.get(oak_node::value::ValueType::Texture)
        else {
            panic!("no texture in the table");
        };
        assert!(!handle.ctx.is_null(), "null texture box");
        (unsafe { oak_node::handle::get_checked::<Texture>(handle) })
            .cloned()
            .expect("value is still a job box (unresolved)")
    }

    /// The helper's guard: a table without a texture channel panics
    /// instead of silently returning a placeholder.
    #[test]
    #[should_panic(expected = "no texture in the table")]
    fn resolved_texture_rejects_a_textureless_table() {
        resolved_texture(&NodeValueTable::default());
    }

    /// A frame-cache job box around `payload`.
    fn cache_job_box(payload: CacheJobPayload) -> NodeValue {
        NodeValue::Texture(oak_node::handle::make_owned(Job::CacheJob(payload)))
    }

    /// A 4x3 F32 frame filled with `rgba`.
    fn cache_test_frame(rgba: [f32; 4]) -> Frame {
        let mut frame = generate_frame(Rational::new(3, 1), (4, 3), PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip(rgba) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        frame
    }

    #[test]
    fn cache_job_reads_the_saved_frame() {
        use oak_node::traverser::RenderHooks;

        let path = cache_job_temp_path("hit");
        let frame = cache_test_frame([0.25, 0.5, 0.75, 1.0]);
        crate::frameio::save_cache_frame(&path, &frame).unwrap();

        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            cache_job_box(CacheJobPayload {
                path: path.clone(),
                time: Rational::new(3, 1),
                fallback: Box::new(NodeValue::None),
            }),
            None,
        );
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(oak_node::id::NodeId::INVALID, &NodeValueRow::new(), &mut table);

        let out = resolved_texture(&table);
        assert_eq!(out.size(), (4, 3));
        assert_eq!(first_pixel(&out), [0.25, 0.5, 0.75, 1.0]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_job_missing_file_substitutes_its_input() {
        use oak_node::traverser::RenderHooks;

        // The file is never written: the load fails and the fallback — a
        // real texture box — must end up in the table.
        let path = cache_job_temp_path("miss");
        let fallback = filled_frame((2, 2), [0.1, 0.2, 0.3, 0.4]);
        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            cache_job_box(CacheJobPayload {
                path,
                time: Rational::new(0, 1),
                fallback: Box::new(NodeValue::Texture(oak_node::handle::make_owned(fallback))),
            }),
            None,
        );
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(oak_node::id::NodeId::INVALID, &NodeValueRow::new(), &mut table);

        let out = resolved_texture(&table);
        assert_eq!(out.size(), (2, 2));
        assert_eq!(first_pixel(&out), [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn nested_cache_job_resolves_through_the_outer_shader_job() {
        use oak_node::traverser::RenderHooks;

        let path = cache_job_temp_path("nested");
        let frame = cache_test_frame([0.5, 0.25, 0.125, 1.0]);
        crate::frameio::save_cache_frame(&path, &frame).unwrap();

        // The outer shader names a type nobody registered, so the pass
        // cannot run and falls back to its effect input — the nested cache
        // job, which must already have resolved to the frame from disk.
        let mut params = NodeValueRow::new();
        params.insert(
            "tex_in".into(),
            cache_job_box(CacheJobPayload {
                path: path.clone(),
                time: Rational::new(3, 1),
                fallback: Box::new(NodeValue::None),
            }),
        );
        let outer = Job::ShaderJob(ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.thisdoesnotexist".into(),
            effect_input: "tex_in".into(),
            params,
            ..ShaderJobPayload::default()
        });
        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(outer)),
            None,
        );
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(oak_node::id::NodeId::INVALID, &NodeValueRow::new(), &mut table);

        let out = resolved_texture(&table);
        assert_eq!(out.size(), (4, 3));
        assert_eq!(first_pixel(&out), [0.5, 0.25, 0.125, 1.0]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn generation_fills_cpu_texture() {
        let mut hooks = RenderEvalHooks::new();
        let mut tex = Texture::wrap_frame(
            generate_frame(Rational::new(1, 1), (8, 8), PixelFormat::F32).unwrap(),
        );
        hooks
            .process_frame_generation(&mut tex, Rational::new(3, 1))
            .unwrap();
        let Texture::Cpu(f) = &tex else {
            unreachable!()
        };
        assert_eq!(f.timestamp, Rational::new(3, 1));
        assert!(f.data.iter().all(|&b| b == 0));
        // GPU destination rejected.
        let mut gpu = Texture::gpu(
            Arc::new(UnusedCtx),
            0,
            8,
            8,
            PixelFormat::F32,
        );
        assert!(hooks
            .process_frame_generation(&mut gpu, Rational::new(1, 1))
            .is_err());
    }

    // The plugin executor lives in a process-wide slot; the tests below
    // mutate it and therefore serialize against each other.
    static PLUGIN_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn plugin_spec() -> JobSpec {
        JobSpec::Plugin {
            instance: 7,
            type_id: "org.oak.test-plugin".into(),
            time: 0.5,
            effect_input_id: Some("Source".into()),
            inputs: Vec::new(),
            values: Vec::new(),
        }
    }

    fn first_pixel(texture: &Texture) -> [f32; 4] {
        // GPU textures are read back for the assertion (tests may take
        // the counted boundary; the playback path never does).
        let frame = texture.to_frame().expect("texture readback");
        let mut out = [0f32; 4];
        for i in 0..4 {
            out[i] = f32::from_le_bytes(frame.data[i * 4..i * 4 + 4].try_into().unwrap());
        }
        out
    }

    #[test]
    fn plugin_job_without_executor_yields_purple_frame() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_executor(None);
        let mut hooks = RenderEvalHooks::new();
        let src = Texture::wrap_frame(
            generate_frame(Rational::new(0, 1), (4, 2), PixelFormat::F32).unwrap(),
        );
        let out = hooks.process_plugin_job(src, &plugin_spec()).unwrap();
        assert_eq!(out.size(), (4, 2));
        assert_eq!(first_pixel(&out), [1.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn plugin_job_executor_error_falls_back_to_purple() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_executor(Some(Arc::new(|_req: &PluginJobRequest<'_>| {
            Err(Error::Failed("boom".into()))
        })));
        let mut hooks = RenderEvalHooks::new();
        let src = Texture::wrap_frame(
            generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap(),
        );
        let out = hooks.process_plugin_job(src, &plugin_spec()).unwrap();
        assert_eq!(first_pixel(&out), [1.0, 0.0, 1.0, 1.0]);
        set_plugin_executor(None);
    }

    /// M3 acceptance: a host that cannot come up (or crashes past its
    /// budget) yields the purple failure frame, exactly like a failing
    /// in-process executor.
    #[test]
    #[cfg(unix)]
    fn plugin_job_host_failure_yields_purple_frame() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        // An executor that would succeed, to prove the host result wins.
        set_plugin_executor(Some(Arc::new(|_req: &PluginJobRequest<'_>| {
            Ok(Texture::wrap_frame(
                generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap(),
            ))
        })));
        let host = crate::ofxhost::OfxHost::new(crate::ofxhost::OfxHostConfig {
            host_bin: Some(std::path::PathBuf::from("/bin/false")),
            max_failures: 1,
            ..Default::default()
        })
        .unwrap();
        crate::ofxhost::install_client(Some(host));
        let mut hooks = RenderEvalHooks::new();
        let src = Texture::wrap_frame(
            generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap(),
        );
        let out = hooks.process_plugin_job(src, &plugin_spec()).unwrap();
        assert_eq!(first_pixel(&out), [1.0, 0.0, 1.0, 1.0]);
        crate::ofxhost::install_client(None);
        set_plugin_executor(None);
    }

    #[test]
    fn plugin_job_dispatches_through_installed_executor() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            // Echo: paint the source size with the instance id.
            let JobSpec::Plugin { instance, .. } = req.spec else {
                return Err(Error::Invalid);
            };
            let v = (*instance as f32) / 10.0;
            let mut frame = generate_frame(Rational::new(0, 1), req.src.size(), PixelFormat::F32)?;
            for pixel in frame.data.chunks_exact_mut(16) {
                for c in 0..4 {
                    pixel[c * 4..c * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            Ok(Texture::wrap_frame(frame))
        })));
        let mut hooks = RenderEvalHooks::new();
        let src = Texture::wrap_frame(
            generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap(),
        );
        let out = hooks.process_plugin_job(src, &plugin_spec()).unwrap();
        assert_eq!(first_pixel(&out), [0.7, 0.7, 0.7, 0.7]);
        set_plugin_executor(None);
    }

    #[test]
    fn resolve_executes_payload_box_and_keeps_plain_textures() {
        use oak_node::nodes::plugin::{PluginInstanceHandle, PluginJobPayload};

        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            let JobSpec::Plugin {
                instance,
                values,
                inputs,
                effect_input_id,
                ..
            } = req.spec
            else {
                return Err(Error::Invalid);
            };
            // The resolve seam must deliver the tagged param values and
            // the clip texture to the executor.
            assert_eq!(*instance, 7);
            assert_eq!(effect_input_id.as_deref(), Some("Source"));
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].0, "Source");
            assert!(values.iter().any(|(k, v)| {
                k == "gain" && matches!(v, NodeValue::Float(f) if (*f - 0.25).abs() < 1e-6)
            }));
            let mut frame = generate_frame(Rational::new(0, 1), req.src.size(), PixelFormat::F32)?;
            for pixel in frame.data.chunks_exact_mut(16) {
                for (i, v) in [0.25f32, 0.5, 0.75, 1.0].iter().enumerate() {
                    pixel[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
            Ok(Texture::wrap_frame(frame))
        })));

        // A real source texture box plus a payload box referencing it.
        let src_frame = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        let src_box = oak_node::handle::make_owned(Texture::wrap_frame(src_frame));
        let mut values = NodeValueRow::new();
        values.insert("Source".into(), NodeValue::Texture(src_box));
        values.insert("gain".into(), NodeValue::Float(0.25));
        let payload = PluginJobPayload {
            instance: PluginInstanceHandle(7),
            type_id: "org.oak.test-plugin".into(),
            time: Rational::new(1, 2),
            effect_input_id: "Source".into(),
            values,
        };

        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(Job::PluginJob(payload))),
            None,
        );

        use oak_node::traverser::RenderHooks;
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(oak_node::id::NodeId::INVALID, &NodeValueRow::new(), &mut table);

        let NodeValue::Texture(handle) = table.get(oak_node::value::ValueType::Texture).unwrap()
        else {
            unreachable!()
        };
        let rendered = unsafe { oak_node::handle::get_checked::<Texture>(handle) }
            .expect("payload box must be replaced by the rendered texture");
        assert_eq!(first_pixel(rendered), [0.25, 0.5, 0.75, 1.0]);
        set_plugin_executor(None);
    }

    /// Stand-in context for the "GPU destination" test (never used for
    /// real GPU work).
    struct UnusedCtx;
    impl oak_core::backend::GpuContextLike for UnusedCtx {
        fn kind(&self) -> oak_core::backend::BackendKind {
            oak_core::backend::BackendKind::Cpu
        }
        fn destroy_texture(&self, _token: u64) {}
        fn upload(&self, _token: u64, _frame: &Frame) -> Result<()> {
            Err(Error::Failed("unused".into()))
        }
        fn download(&self, _token: u64) -> Result<Frame> {
            Err(Error::Failed("unused".into()))
        }
        fn blit(
            &self,
            _src: u64,
            _dst: u64,
            _processor: Option<&oak_core::color::ColorProcessor>,
        ) -> Result<()> {
            Err(Error::Failed("unused".into()))
        }
    }
    use std::sync::Arc;

    // ---- Audio (M12 P1 / M15 S3) -----------------------------------------

    fn audio_params(range: TimeRange) -> crate::ticket::AudioTicketParams {
        crate::ticket::AudioTicketParams {
            viewer: 1,
            range,
            sample_rate: 48000,
            channel_layout: 0x3,
            montage: Vec::new(),
        }
    }

    #[test]
    fn render_audio_samples_produces_silence_for_empty_montage() {
        // M12 P1: an empty montage renders total silence at the requested
        // layout.
        let params = audio_params(TimeRange::new(Rational::new(0, 1), Rational::new(1, 24)));
        match render_audio_samples(&params).unwrap() {
            crate::ticket::TicketPayload::Audio(samples) => {
                // 1/24 s at 48 kHz = 2000 sample frames, stereo.
                assert_eq!(samples.sample_rate, 48000);
                assert_eq!(samples.channel_count, 2);
                assert_eq!(samples.samples.len(), 2000 * 2);
                assert!(samples.samples.iter().all(|&v| v == 0.0), "silence");
            }
            other => panic!("expected Audio payload, got {other:?}"),
        }
    }

    #[test]
    fn render_audio_samples_into_matches_heap_path_byte_for_byte() {
        // M15 S3: the shm-slot writer must produce exactly the same
        // little-endian f32 bytes as the heap path, so a worker's slot and
        // the in-process fallback agree for the same montage.
        let params = audio_params(TimeRange::new(Rational::new(0, 1), Rational::new(1, 48)));
        let heap = match render_audio_samples(&params).unwrap() {
            crate::ticket::TicketPayload::Audio(samples) => samples,
            other => panic!("expected Audio payload, got {other:?}"),
        };
        let mut dst = vec![0u8; heap.samples.len() * 4];
        render_audio_samples_into(&params, &mut dst).unwrap();
        let expected: Vec<u8> = heap
            .samples
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(dst, expected);
        // And the into-path output parses back into the same samples.
        let parsed: Vec<f32> = dst
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(parsed, heap.samples);
    }

    #[test]
    fn render_audio_samples_into_rejects_small_buffer() {
        let params = audio_params(TimeRange::new(Rational::new(0, 1), Rational::new(1, 24)));
        let mut dst = [0u8; 8]; // far too small for 2000x2 f32 samples
        assert!(render_audio_samples_into(&params, &mut dst).is_err());
    }

    // ---- Montage clip effect stacks -------------------------------------

    /// A 2x1 F32 texture filled with a known color.
    fn solid_texture(r: f32, g: f32, b: f32, a: f32) -> Texture {
        let mut frame = generate_frame(Rational::new(0, 1), (2, 1), PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip([r, g, b, a]) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        Texture::Cpu(frame)
    }

    fn opacity_effect(enabled: bool, value: f64) -> crate::ticket::MontageEffect {
        crate::ticket::MontageEffect {
            type_id: OPACITY_EFFECT_TYPE_ID.to_string(),
            enabled,
            effect_input_id: Some("tex_in".to_string()),
            params: vec![(OPACITY_VALUE_INPUT.to_string(), NodeValue::Float(value))],
        }
    }

    fn clip_with_effects(effects: Vec<crate::ticket::MontageEffect>) -> crate::ticket::MontageClip {
        crate::ticket::MontageClip {
            filename: String::new(),
            stream_index: 0,
            in_time: Rational::new(0, 1),
            out_time: Rational::new(1, 1),
            media_in: Rational::new(0, 1),
            gain: 1.0,
            effects,
        }
    }

    /// The built-in Opacity effect really scales the decoded pixels
    /// (every channel, C++ `opacity.frag` parity).
    #[test]
    fn montage_opacity_effect_scales_pixels() {
        let clip = clip_with_effects(vec![opacity_effect(true, 0.5)]);
        let out = apply_clip_effects(solid_texture(0.8, 0.4, 0.2, 1.0), &clip, Rational::new(0, 1));
        assert_eq!(first_pixel(&out), [0.4, 0.2, 0.1, 0.5]);
    }

    /// Disabled effects are bypassed (C++ traverser parity), and unity
    /// opacity is a pass-through.
    #[test]
    fn montage_disabled_or_unity_effects_pass_through() {
        let disabled = clip_with_effects(vec![opacity_effect(false, 0.5)]);
        let out = apply_clip_effects(solid_texture(0.8, 0.4, 0.2, 1.0), &disabled, Rational::new(0, 1));
        assert_eq!(first_pixel(&out), [0.8, 0.4, 0.2, 1.0]);

        let unity = clip_with_effects(vec![opacity_effect(true, 1.0)]);
        let out = apply_clip_effects(solid_texture(0.8, 0.4, 0.2, 1.0), &unity, Rational::new(0, 1));
        assert_eq!(first_pixel(&out), [0.8, 0.4, 0.2, 1.0]);
    }

    /// An OFX effect (any non-built-in type id) resolves its instance
    /// through the installed factory and dispatches through the executor,
    /// carrying the montage's parameter values.
    #[test]
    fn montage_plugin_effect_dispatches_with_params() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        set_plugin_instance_factory(Some(Arc::new(|identifier: &str| {
            (identifier == "com.example.darken").then_some(7)
        })));
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            let JobSpec::Plugin { instance, values, .. } = req.spec else {
                return Err(Error::Invalid);
            };
            assert_eq!(*instance, 7);
            // The injected parameter drives the output: paint the frame
            // with the "gain" value so the test observes the param path.
            let gain = values
                .iter()
                .find(|(k, _)| k == "gain")
                .map(|(_, v)| v.to_double() as f32)
                .unwrap_or(1.0);
            let mut frame = generate_frame(Rational::new(0, 1), req.src.size(), PixelFormat::F32)?;
            for px in frame.data.chunks_exact_mut(16) {
                for (c, v) in px.chunks_exact_mut(4).zip([gain, gain, gain, 1.0]) {
                    c.copy_from_slice(&v.to_le_bytes());
                }
            }
            Ok(Texture::Cpu(frame))
        })));
        let clip = clip_with_effects(vec![crate::ticket::MontageEffect {
            type_id: "com.example.darken".to_string(),
            enabled: true,
            effect_input_id: Some("Source".to_string()),
            params: vec![("gain".to_string(), NodeValue::Float(0.25))],
        }]);
        let out = apply_clip_effects(solid_texture(0.8, 0.4, 0.2, 1.0), &clip, Rational::new(0, 1));
        assert_eq!(first_pixel(&out), [0.25, 0.25, 0.25, 1.0]);
        set_plugin_executor(None);
        set_plugin_instance_factory(None);
    }

    /// An effect nobody can evaluate (no factory / unknown type) passes
    /// the frame through unchanged — loudly (the warn-once log), never a
    /// silent no-op.
    #[test]
    fn montage_unknown_effect_passes_through() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        set_plugin_instance_factory(None);
        let clip = clip_with_effects(vec![crate::ticket::MontageEffect {
            type_id: "com.example.missing".to_string(),
            enabled: true,
            effect_input_id: Some("Source".to_string()),
            params: Vec::new(),
        }]);
        let out = apply_clip_effects(solid_texture(0.8, 0.4, 0.2, 1.0), &clip, Rational::new(0, 1));
        assert_eq!(first_pixel(&out), [0.8, 0.4, 0.2, 1.0]);
    }

    // ---- Graph-driven sequence (M12 phase 2) ----------------------------

    /// Native-size decode: a `(0, 0)` request produces the footage's
    /// intrinsic dimensions (the graph sequence path decodes native and
    /// scales at composite time).
    #[test]
    fn footage_native_size_decode() {
        let path = std::env::temp_dir().join(format!("oakrender_graph_native_{}.mp4", std::process::id()));
        oak_codec::testmedia::write_test_clip(&path, 64, 64, 10, 10).expect("test clip generation");
        let tex = render_footage_frame(&path.to_string_lossy(), 0, Rational::new(0, 1), (0, 0), PixelFormat::F32)
            .expect("native-size decode");
        assert_eq!(tex.size(), (64, 64));
        let _ = std::fs::remove_file(&path);
    }

    /// The resolve seam decodes each boxed [`FootageJobPayload`] into a
    /// texture and leaves genuine texture boxes untouched (in-place row
    /// replacement, C++ FootageJob processing).
    #[test]
    fn resolve_footage_jobs_decodes_payload_box() {
        let path = std::env::temp_dir().join(format!("oakrender_graph_resolve_{}.mp4", std::process::id()));
        oak_codec::testmedia::write_test_clip(&path, 32, 32, 10, 10).expect("test clip generation");

        let mut table = NodeValueTable::default();
        let payload = FootageJobPayload {
            filename: path.to_string_lossy().into_owned(),
            stream_index: 0,
            time: Rational::new(0, 1),
        };
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(Job::FootageJob(payload))),
            None,
        );
        let genuine = Texture::wrap_frame(generate_frame(Rational::new(0, 1), (4, 4), PixelFormat::F32).unwrap());
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(genuine)),
            None,
        );

        use oak_node::traverser::RenderHooks;
        let mut hooks = RenderEvalHooks::new();
        hooks.frame_size = Some((32, 32));
        hooks.resolve(oak_node::id::NodeId::INVALID, &NodeValueRow::new(), &mut table);
        assert_eq!(table.count(), 2, "both rows stay, only the payload is replaced");

        let rows = table.rows();
        let NodeValue::Texture(decoded_handle) = &rows[0].1 else {
            unreachable!()
        };
        let decoded = unsafe { oak_node::handle::get_checked::<Texture>(decoded_handle) }
            .expect("payload box replaced by the decoded texture");
        let Texture::Cpu(frame) = decoded else {
            unreachable!()
        };
        assert_eq!((frame.width, frame.height), (32, 32));
        assert!(
            frame.data.iter().any(|&b| b != 0),
            "decoded frame must contain non-black pixels"
        );

        let NodeValue::Texture(genuine_handle) = &rows[1].1 else {
            unreachable!()
        };
        let genuine = unsafe { oak_node::handle::get_checked::<Texture>(genuine_handle) }
            .expect("genuine texture box stays untouched");
        assert_eq!(genuine.size(), (4, 4));

        let _ = std::fs::remove_file(&path);
    }

    /// The resolve seam applies a ColorTransformJob's OCIO processor for
    /// real (C++ ColorTransformJob processing): a CPU frame converts
    /// through the LUT in place.
    #[test]
    fn resolve_color_transform_job_applies_lut_on_cpu() {
        if oak_core::color::set_up_default_config().is_err() {
            eprintln!("bundled OCIO missing; skipping");
            return;
        }
        // 1D LUT doubling the red channel (linear ramp 0→0, 1→2).
        let path = std::env::temp_dir()
            .join(format!("oakrender_lut_double_{}.cube", std::process::id()));
        std::fs::write(&path, "LUT_1D_SIZE 2\n0.0 0.0 0.0\n2.0 1.0 1.0\n").unwrap();

        let Some(processor) = oak_core::color::ColorProcessor::create_lut(
            path.to_str().unwrap(),
            oak_core::color::Direction::Normal,
        )
        .filter(|p| p.is_valid()) else {
            eprintln!("LUT processor unavailable; skipping");
            let _ = std::fs::remove_file(&path);
            return;
        };

        // Input: a 0.25-grey CPU frame.
        let mut frame = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip([0.25f32, 0.25, 0.25, 1.0]) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        let payload = ColorTransformJobPayload {
            color_processor: std::sync::Arc::new(processor),
            input: NodeValue::Texture(oak_node::handle::make_owned(Texture::wrap_frame(frame))),
            time: Rational::new(0, 1),
        };
        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(Job::ColorTransformJob(payload))),
            None,
        );

        use oak_node::traverser::RenderHooks;
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(
            oak_node::id::NodeId::INVALID,
            &NodeValueRow::new(),
            &mut table,
        );

        let rows = table.rows();
        let NodeValue::Texture(handle) = &rows[0].1 else {
            unreachable!()
        };
        let out = unsafe { oak_node::handle::get_checked::<Texture>(handle) }
            .expect("the job box is replaced by the converted texture");
        let px = first_pixel(out);
        assert!(
            (px[0] - 0.5).abs() < 1e-3,
            "red channel doubles through the LUT: {px:?}"
        );
        assert!((px[1] - 0.25).abs() < 1e-4, "green unchanged: {px:?}");
        assert_eq!(px[3], 1.0, "alpha preserved");
        let _ = std::fs::remove_file(&path);
    }

    /// M2: a ColorTransformJob on a GPU texture bakes the processor into
    /// a 3D LUT and runs the GPU color pass — the transform is applied
    /// (not passed through) and the result stays GPU-resident.
    #[test]
    fn resolve_color_transform_job_applies_lut_on_gpu() {
        if oak_core::color::set_up_default_config().is_err() {
            eprintln!("bundled OCIO missing; skipping");
            return;
        }
        let Some(ctx) = oak_core::backend::shared_gpu_or_skip("an eval GPU test") else {
            return;
        };
        // 1D LUT doubling the red channel (linear ramp 0→0, 1→2).
        let path = std::env::temp_dir()
            .join(format!("oakrender_lut_double_gpu_{}.cube", std::process::id()));
        std::fs::write(&path, "LUT_1D_SIZE 2\n0.0 0.0 0.0\n2.0 1.0 1.0\n").unwrap();
        let Some(processor) = oak_core::color::ColorProcessor::create_lut(
            path.to_str().unwrap(),
            oak_core::color::Direction::Normal,
        )
        .filter(|p| p.is_valid()) else {
            eprintln!("LUT processor unavailable; skipping");
            let _ = std::fs::remove_file(&path);
            return;
        };

        let mut frame = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip([0.25f32, 0.25, 0.25, 1.0]) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        let token = ctx.create_texture(2, 2).unwrap();
        ctx.upload(token, &frame).unwrap();
        let input = Texture::gpu(ctx.clone(), token, 2, 2, PixelFormat::F32);
        let payload = ColorTransformJobPayload {
            color_processor: std::sync::Arc::new(processor),
            input: NodeValue::Texture(oak_node::handle::make_owned(input)),
            time: Rational::new(0, 1),
        };
        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(Job::ColorTransformJob(payload))),
            None,
        );

        use oak_node::traverser::RenderHooks;
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(
            oak_node::id::NodeId::INVALID,
            &NodeValueRow::new(),
            &mut table,
        );

        let rows = table.rows();
        let NodeValue::Texture(handle) = &rows[0].1 else {
            unreachable!()
        };
        let out = unsafe { oak_node::handle::get_checked::<Texture>(handle) }
            .expect("the job box is replaced by the converted texture");
        assert!(
            matches!(out, Texture::Gpu { .. }),
            "the GPU color transform stays on the GPU"
        );
        let px = first_pixel(out);
        assert!(
            (px[0] - 0.5).abs() < 5e-3,
            "red channel doubles through the GPU LUT: {px:?}"
        );
        assert!((px[1] - 0.25).abs() < 5e-3, "green unchanged: {px:?}");
        assert_eq!(px[3], 1.0, "alpha preserved");
        let _ = std::fs::remove_file(&path);
    }

    /// An invalid processor passes the input texture through unchanged
    /// (C++ creates processors non-fatally).
    #[test]
    fn resolve_color_transform_job_passes_through_when_processor_invalid() {
        let mut frame = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip([0.4f32, 0.3, 0.2, 1.0]) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        let payload = ColorTransformJobPayload {
            color_processor: std::sync::Arc::new(
                oak_core::color::ColorProcessor::pass_through(),
            ),
            input: NodeValue::Texture(oak_node::handle::make_owned(Texture::wrap_frame(frame))),
            time: Rational::new(0, 1),
        };
        let mut table = NodeValueTable::default();
        table.push(
            oak_node::value::ValueType::Texture,
            NodeValue::Texture(oak_node::handle::make_owned(Job::ColorTransformJob(payload))),
            None,
        );

        use oak_node::traverser::RenderHooks;
        let mut hooks = RenderEvalHooks::new();
        hooks.resolve(
            oak_node::id::NodeId::INVALID,
            &NodeValueRow::new(),
            &mut table,
        );

        let rows = table.rows();
        let NodeValue::Texture(handle) = &rows[0].1 else {
            unreachable!()
        };
        let out = unsafe { oak_node::handle::get_checked::<Texture>(handle) }
            .expect("the job box is replaced by the input texture");
        assert_eq!(first_pixel(out), [0.4, 0.3, 0.2, 1.0], "untouched");
    }
    #[test]
    fn composite_tracks_matches_alpha_over_math() {
        let frames = vec![
            solid_texture(0.5, 0.25, 0.125, 0.5),
            solid_texture(1.0, 1.0, 1.0, 0.75),
        ];
        let expected = [0.625f32, 0.5, 0.4375, 0.875];

        // bottom over transparent: (0.75, 0.75, 0.75, 0.75), then top over:
        // r = 0.5*0.5 + 0.75*0.5, g = 0.25*0.5 + 0.75*0.5,
        // b = 0.125*0.5 + 0.75*0.5, a = 0.5 + 0.75*0.5.
        // The CPU fallback must match the GPU math (and the layer
        // order); call it directly so the assertion never depends on
        // whether some other test installed a shared GPU context.
        let out = composite_tracks_cpu(&frames, (2, 1));
        let pixel = first_pixel(&out);
        for (got, want) in pixel.iter().zip(expected) {
            assert!((got - want).abs() < 1e-4, "CPU composite: expected {want}, got {got}");
        }

        // Same math through the GPU pass when a device is available.
        if let Some(ctx) = oak_core::backend::GpuContext::shared() {
            let gpu_out = composite_tracks_gpu(&ctx, &frames, (2, 1)).expect("GPU composite");
            let pixel = first_pixel(&gpu_out);
            for (got, want) in pixel.iter().zip(expected) {
                assert!((got - want).abs() < 1e-3, "GPU composite: expected {want}, got {got}");
            }
        }
    }

    /// End-to-end chromakey through the real GPU path: a solid opaque
    /// green frame keyed on the node's default green key leaves every
    /// pixel at zero (the C++ `ColorTransformJob` + `chromakey.frag`
    /// math: `colorclose` returns 0 at the key color, so the
    /// shadows/highlights transform yields `mask = 0` and `col *= mask`
    /// blanks the frame). Exercises the OCIO splice in
    /// [`super::process_shader_job`]: the shader's `%1` marker is filled
    /// with the real `SceneLinearToCIEXYZ_d65` GLSL generated from the
    /// default OCIO config, and the tolerance uniforms reach the shader
    /// under their (fixed) input-id spelling.
    #[test]
    fn gpu_chromakey_keys_green_with_ocio_stub() {
        let Some(ctx) = oak_core::backend::shared_gpu_or_skip("an eval GPU test") else {
            return;
        };
        // Install the process-wide default config (the C++
        // `ColorManager::SetUpDefaultConfig` startup step; color.rs tests
        // do the same). Without it the OCIO stub cannot be generated and
        // the job falls back to the input pass-through.
        if oak_core::color::set_up_default_config().is_err() {
            eprintln!("bundled OCIO missing; skipping");
            return;
        }
        if oak_core::color::ocio_function_shader(
            "SceneLinearToCIEXYZ_d65",
            "scene_linear",
            "cie_xyz_d65_interchange",
        )
            .is_none()
        {
            eprintln!("no OCIO config; skipping");
            return;
        }

        let size = (16, 16);
        let mut frame = generate_frame(Rational::new(0, 1), size, PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip([0.0f32, 1.0, 0.0, 1.0]) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }

        let src = ctx.create_texture(size.0, size.1).unwrap();
        ctx.upload(src, &frame).unwrap();
        let input = Texture::gpu(ctx.clone(), src, size.0, size.1, PixelFormat::F32);

        let mut params = NodeValueRow::new();
        params.insert("tex_in".into(), NodeValue::Texture(oak_node::handle::make_owned(input)));
        params.insert("color_key".into(), NodeValue::Color([0.0, 1.0, 0.0, 1.0]));
        params.insert("lower_tolerance_in".into(), NodeValue::Float(5.0));
        params.insert("upper_tolerance_in".into(), NodeValue::Float(25.0));
        params.insert("mask_only_in".into(), NodeValue::Boolean(false));
        params.insert("invert_in".into(), NodeValue::Boolean(false));
        params.insert("shadows_in".into(), NodeValue::Float(100.0));
        params.insert("highlights_in".into(), NodeValue::Float(100.0));

        let payload = ShaderJobPayload {
            node_id: oak_node::id::NodeId::from_identity(1).unwrap(),
            time: Rational::new(0, 1),
            iterations: 1,
            type_id: "org.olivevideoeditor.Olive.chromakey".into(),
            shader_id: String::new(),
            effect_input: "tex_in".into(),
            params,
            iterative_input: String::new(),
        };
        let rendered = RenderEvalHooks::new()
            .process_shader_job(&payload)
            .expect("chromakey renders on the GPU");

        let Texture::Gpu { token: dst, .. } = rendered else {
            panic!("expected a GPU texture");
        };
        let out = ctx.download(dst).unwrap();
        for px in out.data.chunks_exact(16) {
            for (c, v) in px.chunks_exact(4).enumerate() {
                let got = f32::from_le_bytes(v.try_into().unwrap());
                assert!(
                    got.abs() < 1e-4,
                    "channel {c}: keyed green must be fully transparent (got {got})"
                );
            }
        }
        ctx.destroy_texture(dst);
        ctx.destroy_texture(src);
    }

    /// Pixel readback helper for the GPU verification tests.
    fn pixel_at(frame: &Frame, x: usize, y: usize) -> [f32; 4] {
        let at = (y * frame.width as usize + x) * 16;
        let mut out = [0f32; 4];
        for c in 0..4 {
            out[c] = f32::from_le_bytes(frame.data[at + c * 4..at + c * 4 + 4].try_into().unwrap());
        }
        out
    }

    /// Build a solid-color F32 CPU frame.
    fn filled_frame(size: (i32, i32), rgba: [f32; 4]) -> Texture {
        let mut frame = generate_frame(Rational::new(0, 1), size, PixelFormat::F32).unwrap();
        for px in frame.data.chunks_exact_mut(16) {
            for (c, v) in px.chunks_exact_mut(4).zip(rgba) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        Texture::wrap_frame(frame)
    }

    /// Evaluate one node's `value()` against `inputs` and resolve the
    /// resulting table through the hooks (the full value -> job -> GPU
    /// run -> texture path), reading the frame back while the table —
    /// which owns the texture's GPU token — is still alive.
    fn eval_node_row(
        type_id: &str,
        inputs: NodeValueRow,
        frame_size: Option<(i32, i32)>,
    ) -> Frame {
        use oak_node::traverser::RenderHooks;
        let (core, behavior) = oak_node::factory::Factory::global()
            .create_any(type_id)
            .expect("node type registered");
        let mut table = NodeValueTable::default();
        behavior.value(&core, &inputs, Rational::new(0, 1), &mut table);
        let mut hooks = RenderEvalHooks::new();
        hooks.frame_size = frame_size;
        hooks.resolve(oak_node::id::NodeId::INVALID, &inputs, &mut table);
        let Some(NodeValue::Texture(handle)) =
            table.get(oak_node::value::ValueType::Texture)
        else {
            panic!("{type_id}: no texture produced");
        };
        if handle.ctx.is_null() {
            panic!("{type_id}: null texture produced");
        }
        let tex = (unsafe { oak_node::handle::get_checked::<Texture>(handle) })
            .expect("resolved texture");
        assert!(
            matches!(tex, Texture::Gpu { .. }),
            "{type_id}: the job must render on the GPU"
        );
        tex.to_frame().expect("readback")
    }

    /// Merge over the real GPU path: the merge node declares no effect
    /// input, so base and blend must bind by name for the alpha-over to
    /// run at all. Red base + half-alpha green blend -> (0.5, 1, 0, 1).
    #[test]
    fn gpu_merge_alpha_over_binds_base_and_blend() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert(
            "base_in".into(),
            texture_value(filled_frame((16, 16), [1.0, 0.0, 0.0, 1.0])),
        );
        inputs.insert(
            "blend_in".into(),
            texture_value(filled_frame((16, 16), [0.0, 1.0, 0.0, 0.5])),
        );
        let frame = eval_node_row("org.olivevideoeditor.Olive.merge", inputs, None);
        assert_eq!(frame.width, 16, "the pass size follows the base");
        for (x, y) in [(0, 0), (8, 8), (15, 15)] {
            let px = pixel_at(&frame, x, y);
            let want = [0.5, 1.0, 0.0, 1.0];
            for (c, (got, w)) in px.iter().zip(want).enumerate() {
                assert!(
                    (got - w).abs() < 1e-4,
                    "merge ({x},{y}) ch{c}: got {got}, want {w}"
                );
            }
        }
    }

    /// A bare generator (no input connected) renders at the hook's frame
    /// size instead of a 1x1 the composite step would drop.
    #[test]
    fn gpu_generator_without_input_renders_at_frame_size() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("color_in".into(), NodeValue::Color([0.2, 0.4, 0.6, 1.0]));
        let frame = eval_node_row(
            "org.olivevideoeditor.Olive.solidgenerator",
            inputs,
            Some((8, 4)),
        );
        assert_eq!((frame.width, frame.height), (8, 4));
        let px = pixel_at(&frame, 3, 2);
        for (c, w) in [0.2f32, 0.4, 0.6, 1.0].iter().enumerate() {
            assert!(
                (px[c] - w).abs() < 1e-4,
                "solid ch{c}: got {}, want {w}",
                px[c]
            );
        }
    }

    /// Generator-over-base ("mrg"): the nested generator job resolves
    /// recursively and alpha-overs onto the base — the pentagon is green
    /// (the generated layer), the corners stay red (the base).
    #[test]
    fn gpu_generator_over_base_composites_nested_job() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert(
            "base_in".into(),
            texture_value(filled_frame((512, 512), [1.0, 0.0, 0.0, 1.0])),
        );
        inputs.insert("color_in".into(), NodeValue::Color([0.0, 1.0, 0.0, 1.0]));
        let frame = eval_node_row("org.olivevideoeditor.Olive.polygon", inputs, Some((512, 512)));
        assert_eq!((frame.width, frame.height), (512, 512));
        let center = pixel_at(&frame, 256, 256);
        assert!(
            center[1] > 0.9 && center[0] < 0.1,
            "pentagon center is the generated green: {center:?}"
        );
        let corner = pixel_at(&frame, 5, 5);
        assert!(
            corner[0] > 0.9 && corner[1] < 0.1,
            "corner keeps the red base: {corner:?}"
        );
    }

    /// Drop shadow with non-zero softness: three iterations feed back
    /// through `previous_iteration_in`; the blurred shadow lands offset
    /// from the source, widening the non-transparent area.
    #[test]
    fn gpu_dropshadow_softness_blurs_and_offsets() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        // 16x16 transparent frame with an opaque 4x4 square at (4,4).
        let mut frame = generate_frame(Rational::new(0, 1), (16, 16), PixelFormat::F32).unwrap();
        for y in 4..8usize {
            for x in 4..8usize {
                let at = (y * 16 + x) * 16;
                for (c, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
                    frame.data[at + c * 4..at + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(Texture::wrap_frame(frame)));
        inputs.insert("color_in".into(), NodeValue::Color([0.0, 0.0, 0.0, 1.0]));
        inputs.insert("distance_in".into(), NodeValue::Float(4.0));
        inputs.insert("angle_in".into(), NodeValue::Float(45.0));
        inputs.insert("radius_in".into(), NodeValue::Float(2.0));
        inputs.insert("opacity_in".into(), NodeValue::Float(1.0));
        inputs.insert("fast_in".into(), NodeValue::Boolean(false));

        let out_frame = eval_node_row("org.olivevideoeditor.Olive.dropshadow", inputs, None);
        assert_eq!((out_frame.width, out_frame.height), (16, 16));
        let covered = out_frame
            .data
            .chunks_exact(16)
            .filter(|px| f32::from_le_bytes(px[12..16].try_into().unwrap()) > 0.01)
            .count();
        assert!(
            covered > 16,
            "the offset blurred shadow must widen the covered area beyond the 4x4 source square: {covered}"
        );
    }
    /// Transform over the real GPU path: the fragment-side inverse
    /// sampling applies the node's matrix for real — a +3px x
    /// translation moves the white pixel from (2, 3) to (5, 3).
    #[test]
    fn gpu_transform_translates_pixels() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        // 8x8 black frame with one white pixel at (2, 3).
        let mut frame = generate_frame(Rational::new(0, 1), (8, 8), PixelFormat::F32).unwrap();
        let at = (3 * 8 + 2) * 16;
        for (c, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
            frame.data[at + c * 4..at + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(Texture::wrap_frame(frame)));
        inputs.insert("pos_in".into(), NodeValue::Vec2([3.0, 0.0]));

        let out = eval_node_row("org.olivevideoeditor.Olive.transform", inputs, None);
        assert_eq!((out.width, out.height), (8, 8));
        assert_eq!(pixel_at(&out, 2, 3), [0.0, 0.0, 0.0, 0.0], "source spot vacated");
        assert_eq!(pixel_at(&out, 5, 3), [1.0, 1.0, 1.0, 1.0], "pixel moved +3 in x");
        assert_eq!(pixel_at(&out, 0, 0), [0.0, 0.0, 0.0, 0.0]);
    }

    /// Transform rotation pivots around the FRAME CENTER (the C++
    /// center-origin pixel space), not the top-left corner: a 90° turn
    /// moves a pixel sitting 2px right of center to 2px below center,
    /// with no scaling or smearing (the turn is lossless).
    #[test]
    fn gpu_transform_rotates_around_the_frame_center() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        // 8x8 black frame with one white pixel at (6, 4) — center+(2, 0).
        let mut frame = generate_frame(Rational::new(0, 1), (8, 8), PixelFormat::F32).unwrap();
        let at = (4 * 8 + 6) * 16;
        for (c, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
            frame.data[at + c * 4..at + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(Texture::wrap_frame(frame)));
        inputs.insert("rot_in".into(), NodeValue::Float(90.0));

        let out = eval_node_row("org.olivevideoeditor.Olive.transform", inputs, None);
        assert_eq!(pixel_at(&out, 6, 4), [0.0, 0.0, 0.0, 0.0], "source spot vacated");
        assert_eq!(
            pixel_at(&out, 3, 6),
            [1.0, 1.0, 1.0, 1.0],
            "90° around (4,4) maps texel center (6.5,4.5) -> (3.5,6.5)"
        );
        let lit = out
            .data
            .chunks_exact(16)
            .filter(|px| f32::from_le_bytes(px[12..16].try_into().unwrap()) > 0.01)
            .count();
        assert_eq!(lit, 1, "a pure rotation neither scales nor smears: {lit} lit pixels");
    }

    /// Transform scale pivots around the frame center too: a center 2x2
    /// block at 2x uniform scale grows into the surrounding 4x4 (a
    /// corner pivot would drag it toward the bottom-right instead).
    #[test]
    fn gpu_transform_scales_around_the_frame_center() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        // 8x8 black frame with a white 2x2 block at texels (3..4, 3..4).
        let mut frame = generate_frame(Rational::new(0, 1), (8, 8), PixelFormat::F32).unwrap();
        for (x, y) in [(3usize, 3usize), (4, 3), (3, 4), (4, 4)] {
            let at = (y * 8 + x) * 16;
            for (c, v) in [1.0f32, 1.0, 1.0, 1.0].iter().enumerate() {
                frame.data[at + c * 4..at + c * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(Texture::wrap_frame(frame)));
        inputs.insert("scale_in".into(), NodeValue::Vec2([2.0, 2.0]));

        let out = eval_node_row("org.olivevideoeditor.Olive.transform", inputs, None);
        // Pivot check via the alpha distribution: the block [3,5] scaled
        // 2x around (4,4) grows symmetrically to [2,6] — the centroid
        // stays at the frame center. A corner pivot would drag the block
        // to [6,10], shifting the centroid off-center and clipping the
        // block against the frame edge.
        let mut total = 0.0f32;
        let mut cx = 0.0f32;
        let mut cy = 0.0f32;
        for y in 0..8usize {
            for x in 0..8usize {
                let a = pixel_at(&out, x, y)[3];
                total += a;
                cx += (x as f32 + 0.5) * a;
                cy += (y as f32 + 0.5) * a;
            }
        }
        let (cx, cy) = (cx / total, cy / total);
        assert!(
            (cx - 4.0).abs() < 0.2 && (cy - 4.0).abs() < 0.2,
            "the block grows around the frame center, centroid ({cx}, {cy})"
        );
    }

    /// A transform that pushes content past the frame edge leaves the
    /// vacated region TRANSPARENT (no edge-pixel smearing): translating
    /// everything +100px in x empties the frame entirely, and a +3px
    /// translation vacates exactly the left three columns.
    #[test]
    fn gpu_transform_off_frame_is_transparent() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let white = filled_frame((8, 8), [1.0, 1.0, 1.0, 1.0]);
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(white));
        inputs.insert("pos_in".into(), NodeValue::Vec2([100.0, 0.0]));
        let out = eval_node_row("org.olivevideoeditor.Olive.transform", inputs, None);
        for y in 0..8usize {
            for x in 0..8usize {
                assert_eq!(
                    pixel_at(&out, x, y),
                    [0.0, 0.0, 0.0, 0.0],
                    "off-frame content must be transparent, got {:?} at ({x},{y})",
                    pixel_at(&out, x, y)
                );
            }
        }

        let white = filled_frame((8, 8), [1.0, 1.0, 1.0, 1.0]);
        let mut inputs = NodeValueRow::new();
        inputs.insert("tex_in".into(), texture_value(white));
        inputs.insert("pos_in".into(), NodeValue::Vec2([3.0, 0.0]));
        let out = eval_node_row("org.olivevideoeditor.Olive.transform", inputs, None);
        for x in 0..3usize {
            assert_eq!(
                pixel_at(&out, x, 4),
                [0.0, 0.0, 0.0, 0.0],
                "the vacated left columns are transparent"
            );
        }
        assert_eq!(pixel_at(&out, 7, 4), [1.0, 1.0, 1.0, 1.0], "the rightmost column keeps content");
    }

    /// Shape generator over the real GPU path: a centered 8x8 rectangle
    /// on a 16x16 frame fills exactly the middle block (pixel centers
    /// with texcoord in [0.25, 0.75)), everything outside stays
    /// transparent.
    #[test]
    fn gpu_shape_rectangle_draws_centered_block() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert("pos_in".into(), NodeValue::Vec2([0.0, 0.0]));
        inputs.insert("size_in".into(), NodeValue::Vec2([8.0, 8.0]));
        inputs.insert("color_in".into(), NodeValue::Color([1.0, 0.0, 0.0, 1.0]));
        inputs.insert("type_in".into(), NodeValue::Combo(0));
        inputs.insert("radius_in".into(), NodeValue::Float(20.0));

        let frame = eval_node_row("org.olivevideoeditor.Olive.shape", inputs, Some((16, 16)));
        assert_eq!((frame.width, frame.height), (16, 16));
        for (x, y, inside) in [(8, 8, true), (4, 4, true), (11, 11, true), (0, 0, false), (3, 8, false), (12, 8, false), (15, 15, false)] {
            let px = pixel_at(&frame, x, y);
            if inside {
                assert_eq!(px, [1.0, 0.0, 0.0, 1.0], "({x},{y}) inside the rect");
            } else {
                assert_eq!(px, [0.0, 0.0, 0.0, 0.0], "({x},{y}) outside the rect");
            }
        }
    }

    /// Shape generator, the ellipse and rounded-rectangle dispatches:
    /// the ellipse fills the center and fades out before the corners;
    /// the rounded rect fills the middle but cuts the corner at (4,4)
    /// (radius 20 clamps to half the 8px size).
    #[test]
    fn gpu_shape_ellipse_and_rounded_rect() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let base_inputs = || {
            let mut inputs = NodeValueRow::new();
            inputs.insert("pos_in".into(), NodeValue::Vec2([0.0, 0.0]));
            inputs.insert("size_in".into(), NodeValue::Vec2([8.0, 8.0]));
            inputs.insert("color_in".into(), NodeValue::Color([1.0, 0.0, 0.0, 1.0]));
            inputs.insert("radius_in".into(), NodeValue::Float(20.0));
            inputs
        };

        let mut ellipse = base_inputs();
        ellipse.insert("type_in".into(), NodeValue::Combo(1));
        let frame = eval_node_row("org.olivevideoeditor.Olive.shape", ellipse, Some((16, 16)));
        assert_eq!(pixel_at(&frame, 8, 8), [1.0, 0.0, 0.0, 1.0], "ellipse center");
        assert_eq!(pixel_at(&frame, 0, 0), [0.0, 0.0, 0.0, 0.0], "ellipse corner faded out");

        let mut rounded = base_inputs();
        rounded.insert("type_in".into(), NodeValue::Combo(2));
        let frame = eval_node_row("org.olivevideoeditor.Olive.shape", rounded, Some((16, 16)));
        assert_eq!(pixel_at(&frame, 8, 8), [1.0, 0.0, 0.0, 1.0], "rounded rect middle");
        assert_eq!(pixel_at(&frame, 6, 6), [1.0, 0.0, 0.0, 1.0], "rounded rect inside the corner arc");
        assert_eq!(pixel_at(&frame, 0, 0), [0.0, 0.0, 0.0, 0.0], "rounded rect far corner");
        assert!(
            pixel_at(&frame, 4, 4)[3] < 0.1,
            "rounded rect corner (4,4) is cut by the arc: {:?}",
            pixel_at(&frame, 4, 4)
        );
    }

    /// Despill over the real GPU path: green-screen AVERAGE caps the
    /// green channel at the red/blue average (the shader's method
    /// dispatch must survive translation).
    #[test]
    fn gpu_despill_average_caps_green() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let mut inputs = NodeValueRow::new();
        inputs.insert(
            "tex_in".into(),
            texture_value(filled_frame((4, 4), [0.2, 0.9, 0.3, 1.0])),
        );
        inputs.insert("color_in".into(), NodeValue::Combo(0));
        inputs.insert("method_in".into(), NodeValue::Combo(0));
        inputs.insert("preserve_luminance_input".into(), NodeValue::Boolean(false));

        let frame = eval_node_row("org.olivevideoeditor.Olive.despill", inputs, None);
        assert_eq!((frame.width, frame.height), (4, 4));
        let px = pixel_at(&frame, 2, 2);
        let want = [0.2f32, 0.25, 0.3, 1.0];
        for (c, w) in want.iter().enumerate() {
            assert!(
                (px[c] - w).abs() < 1e-4,
                "despill ch{c}: got {}, want {w}",
                px[c]
            );
        }
    }

    // ---- Endpoint-anchored BFS sweep (M0b) ------------------------------

    /// The M0b integration path over real media and a real effect: a test
    /// clip probed into a footage node feeds `GraphInput.feed_in`, a
    /// Position node sits between the endpoints, and `eval_graph_bfs`
    /// decodes the frame, runs the effect pass and returns the output
    /// endpoint's texture. The Position offset is `(16, 0)`, so the frame
    /// must come out shifted right by 16 pixels — the pixels alone prove
    /// both the decode and the shader pass ran inside the sweep.
    #[test]
    fn bfs_endpoint_sweep_renders_footage_through_position() {
        use oak_node::nodes::graphendpoints::{GRAPH_INPUT_FEED_INPUT, GRAPH_OUTPUT_INPUT};

        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }

        let path = std::env::temp_dir().join(format!(
            "oakrender_bfs_endpoints_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip_solid(
            &path,
            64,
            64,
            10,
            10,
            [0.9, 0.2, 0.1, 1.0],
        )
        .expect("test clip generation");

        let mut graph = oak_node::graph::Graph::new();
        let (input, output) = graph.ensure_endpoints();
        // The default input -> output edge is replaced by the effect chain.
        graph.disconnect(input, output, GRAPH_OUTPUT_INPUT, -1);

        let (core, _) = oak_node::footage::FootageBehavior::create();
        let mut footage = oak_node::footage::FootageBehavior::new(&path.to_string_lossy());
        footage.probe().expect("probe the test clip");
        let footage = graph.add_node(core, Box::new(footage));

        let (core, behavior) = oak_node::factory::Factory::global()
            .create_any("org.olivevideoeditor.Olive.position")
            .expect("position node registered");
        let position = graph.add_node(core, behavior);
        graph
            .get_mut(position)
            .expect("the position node is live")
            .core
            .set_standard_value("offset_in", -1, NodeValue::Vec2([16.0, 0.0]));

        graph
            .connect(footage, input, GRAPH_INPUT_FEED_INPUT, -1)
            .expect("footage -> GraphInput.feed_in");
        graph
            .connect(input, position, "tex_in", -1)
            .expect("GraphInput.tex_out -> position.tex_in");
        graph
            .connect(position, output, GRAPH_OUTPUT_INPUT, -1)
            .expect("position.tex_out -> GraphOutput.tex_in");

        let mut hooks = RenderEvalHooks::new();
        hooks.frame_size = Some((64, 64));
        let value = oak_node::traverser::Traverser::new()
            .eval_graph_bfs(&graph, Rational::new(0, 1), &mut hooks)
            .expect("the endpoint sweep runs");

        let NodeValue::Texture(handle) = value else {
            panic!("the sweep must return the output endpoint's texture");
        };
        assert!(!handle.ctx.is_null(), "the returned box is still a job");
        let texture = (unsafe { oak_node::handle::get_checked::<Texture>(&handle) })
            .cloned()
            .expect("the returned box holds a Texture");
        assert_eq!(texture.size(), (64, 64));
        let frame = texture.to_frame().expect("readback");

        let reference = render_footage_frame(
            &path.to_string_lossy(),
            0,
            Rational::new(0, 1),
            (64, 64),
            PixelFormat::F32,
        )
        .expect("reference decode");
        let reference = reference.to_frame().expect("reference readback");
        let reference_px = pixel_at(&reference, 32, 32);
        assert!(
            reference_px[0] > 0.3 && reference_px[0] > 4.0 * reference_px[1],
            "the reference clip is red-dominant: {reference_px:?}"
        );

        // The vacated left columns are transparent, not a clamped edge
        // column (the Position node's whole-pixel translation).
        for (x, y) in [(0, 0), (8, 32), (15, 63)] {
            assert!(
                pixel_at(&frame, x, y)[3] < 1e-4,
                "({x},{y}) must be transparent after the shift: {:?}",
                pixel_at(&frame, x, y)
            );
        }
        // The footage content arrives at (x + 16, y).
        for (x, y) in [(16, 0), (32, 32), (63, 63)] {
            let got = pixel_at(&frame, x, y);
            let want = pixel_at(&reference, x - 16, y);
            for (c, (g, w)) in got.iter().zip(want).enumerate() {
                assert!(
                    (g - w).abs() < 1e-3,
                    "shifted pixel ({x},{y}) ch{c}: got {g}, want {w}"
                );
            }
        }

        let _ = std::fs::remove_file(&path);
    }

    // ---- Coverage edges: the eval seam's error and fallback paths --------

    use oak_core::handle::CHandle;
    use oak_node::project::Project;

    /// A GPU-like context whose readback returns a fixed frame: the
    /// non-wgpu fallback and the foreign-context readback paths need a
    /// `Texture::Gpu` without a real device.
    struct FrameEchoCtx {
        frame: Frame,
    }

    impl oak_core::backend::GpuContextLike for FrameEchoCtx {
        fn kind(&self) -> oak_core::backend::BackendKind {
            oak_core::backend::BackendKind::Cpu
        }
        fn destroy_texture(&self, _token: u64) {}
        fn upload(&self, _token: u64, _frame: &Frame) -> Result<()> {
            Ok(())
        }
        fn download(&self, _token: u64) -> Result<Frame> {
            Ok(self.frame.clone())
        }
        fn blit(
            &self,
            _src: u64,
            _dst: u64,
            _processor: Option<&oak_core::color::ColorProcessor>,
        ) -> Result<()> {
            Err(Error::Failed("FrameEchoCtx cannot blit".into()))
        }
    }

    /// A `Texture::Gpu` on [`FrameEchoCtx`] reporting `size`.
    fn echo_gpu_texture(size: (i32, i32), rgba: [f32; 4], token: u64) -> Texture {
        let frame = filled_frame(size, rgba)
            .to_frame()
            .unwrap_or_else(|_| Frame::dummy());
        Texture::gpu(
            Arc::new(FrameEchoCtx { frame }),
            token,
            size.0,
            size.1,
            PixelFormat::F32,
        )
    }

    /// An imported planar YUV texture on the non-wgpu stand-in context
    /// (the real decode path never produces one in these tests).
    fn planar_texture(size: (i32, i32)) -> Texture {
        Texture::wrap_planar(oak_core::texture::PlanarTexture::new(
            Arc::new(UnusedCtx),
            oak_core::texture::PlanarFormat::Nv12,
            size,
            (1, 2),
            oak_core::backend::YuvTransform::bt709_limited(),
            (2, 2),
        ))
    }

    /// A valid OCIO processor (a 1D LUT doubling red) or `None` (with a
    /// printed reason) when the bundled config is unavailable.
    fn red_doubling_processor(tag: &str) -> Option<oak_core::color::ColorProcessor> {
        lut_processor(tag, 2.0)
    }

    /// A valid OCIO processor for the 1D LUT `0 -> 0`, `1 -> scale`.
    fn lut_processor(tag: &str, scale: f32) -> Option<oak_core::color::ColorProcessor> {
        if oak_core::color::set_up_default_config().is_err() {
            eprintln!("{tag}: bundled OCIO missing; skipping");
            return None;
        }
        let path = std::env::temp_dir().join(format!(
            "oakrender_eval_{tag}_{}.cube",
            std::process::id()
        ));
        std::fs::write(
            &path,
            format!("LUT_1D_SIZE 2\n0.0 0.0 0.0\n{scale} 1.0 1.0\n"),
        )
        .ok()?;
        let processor = oak_core::color::ColorProcessor::create_lut(
            path.to_str()?,
            oak_core::color::Direction::Normal,
        )
        .filter(|p| p.is_valid());
        let _ = std::fs::remove_file(&path);
        if processor.is_none() {
            eprintln!("{tag}: LUT processor unavailable; skipping");
        }
        processor
    }

    #[test]
    fn hooks_default_disables_cache_and_reports_it() {
        use oak_node::traverser::RenderHooks;
        let hooks = RenderEvalHooks::default();
        assert!(!hooks.use_cache());
        let mut on = RenderEvalHooks::new();
        on.use_cache = true;
        assert!(on.use_cache());
        assert!(on.ticket.is_none() && on.frame_size.is_none());
    }

    #[test]
    fn color_transform_job_rejects_non_texture_and_null_inputs() {
        let processor = Arc::new(oak_core::color::ColorProcessor::pass_through());
        let mut hooks = RenderEvalHooks::new();

        let payload = ColorTransformJobPayload {
            color_processor: processor.clone(),
            input: NodeValue::Float(1.0),
            time: Rational::new(0, 1),
        };
        assert!(matches!(
            hooks.process_color_transform_job(&payload),
            Err(Error::Invalid)
        ));

        let payload = ColorTransformJobPayload {
            color_processor: processor,
            input: NodeValue::Texture(CHandle::null()),
            time: Rational::new(0, 1),
        };
        assert!(matches!(
            hooks.process_color_transform_job(&payload),
            Err(Error::Invalid)
        ));
    }

    #[test]
    fn color_transform_job_passes_planar_textures_through() {
        let Some(processor) = red_doubling_processor("planar") else {
            return;
        };
        let payload = ColorTransformJobPayload {
            color_processor: Arc::new(processor),
            input: NodeValue::Texture(oak_node::handle::make_owned(planar_texture((2, 2)))),
            time: Rational::new(0, 1),
        };
        let out = RenderEvalHooks::new()
            .process_color_transform_job(&payload)
            .expect("planar pass-through");
        assert!(out.is_planar());
    }

    #[test]
    fn color_transform_job_without_a_wgpu_context_converts_on_cpu() {
        let Some(processor) = red_doubling_processor("ctxfallback") else {
            return;
        };
        let input = echo_gpu_texture((2, 2), [0.25, 0.25, 0.25, 1.0], 4242);
        let payload = ColorTransformJobPayload {
            color_processor: Arc::new(processor),
            input: NodeValue::Texture(oak_node::handle::make_owned(input)),
            time: Rational::new(0, 1),
        };
        let out = RenderEvalHooks::new()
            .process_color_transform_job(&payload)
            .expect("cpu fallback");
        assert!(matches!(out, Texture::Cpu(_)), "the fallback wraps a CPU frame");
        let px = first_pixel(&out);
        assert!((px[0] - 0.5).abs() < 1e-3, "red doubled on the CPU: {px:?}");
    }

    #[test]
    fn plugin_job_rejects_a_non_plugin_spec() {
        let mut hooks = RenderEvalHooks::new();
        let src = Texture::wrap_frame(
            generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap(),
        );
        assert!(matches!(
            hooks.process_plugin_job(src, &JobSpec::Generate),
            Err(Error::Invalid)
        ));
    }

    #[test]
    fn resolve_value_guards_depth_null_and_in_flight() {
        let mut hooks = RenderEvalHooks::new();
        let mut in_flight = HashSet::new();

        // Depth ceiling: even a job box is left untouched.
        let mut deep = NodeValue::Texture(oak_node::handle::make_owned(Job::FootageJob(
            FootageJobPayload::default(),
        )));
        hooks.resolve_value(&mut deep, 64, &mut in_flight);
        let NodeValue::Texture(deep_handle) = &deep else {
            unreachable!()
        };
        assert!(unsafe { oak_node::jobs::job_ref(deep_handle) }.is_some());
        assert!(in_flight.is_empty());

        // A non-texture value passes through untouched.
        let mut scalar = NodeValue::Float(0.5);
        hooks.resolve_value(&mut scalar, 0, &mut in_flight);
        assert!(matches!(scalar, NodeValue::Float(v) if v == 0.5));

        // Empty handle: nothing to resolve.
        let mut empty = NodeValue::Texture(CHandle::null());
        hooks.resolve_value(&mut empty, 0, &mut in_flight);
        assert!(matches!(empty, NodeValue::Texture(_)));

        // A handle already in flight is skipped (the cycle guard).
        let mut boxed = NodeValue::Texture(oak_node::handle::make_owned(Job::FootageJob(
            FootageJobPayload::default(),
        )));
        let key = match &boxed {
            NodeValue::Texture(h) => h.ctx as usize,
            _ => unreachable!(),
        };
        in_flight.insert(key);
        hooks.resolve_value(&mut boxed, 0, &mut in_flight);
        let NodeValue::Texture(still) = &boxed else {
            unreachable!()
        };
        assert!(unsafe { oak_node::jobs::job_ref(still) }.is_some());
        assert!(in_flight.contains(&key));
    }

    #[test]
    fn footage_job_with_missing_media_keeps_its_box() {
        let payload = FootageJobPayload {
            filename: std::env::temp_dir()
                .join(format!("oakrender_missing_{}.mp4", std::process::id()))
                .to_string_lossy()
                .into_owned(),
            stream_index: 0,
            time: Rational::new(0, 1),
        };
        assert!(RenderEvalHooks::new()
            .process_footage_job_value(&payload)
            .is_none());
    }

    #[test]
    fn plugin_job_value_splits_clip_inputs_from_scalar_values() {
        use oak_node::nodes::plugin::{PluginInstanceHandle, PluginJobPayload};

        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            let JobSpec::Plugin {
                effect_input_id,
                inputs,
                values,
                ..
            } = req.spec
            else {
                return Err(Error::Invalid);
            };
            assert!(effect_input_id.is_none(), "empty id maps to None");
            assert_eq!(inputs.len(), 1, "only the genuine texture box is a clip");
            assert_eq!(inputs[0].0, "Source");
            assert!(values.iter().any(|(k, _)| k == "gain"));
            assert!(values.iter().all(|(k, _)| k != "Nothing"));
            let mut frame =
                generate_frame(Rational::new(0, 1), req.src.size(), PixelFormat::F32)?;
            for pixel in frame.data.as_chunks_mut::<16>().0 {
                for (c, v) in pixel
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip([0.5f32, 0.25, 0.125, 1.0])
                {
                    c.copy_from_slice(&v.to_le_bytes());
                }
            }
            Ok(Texture::wrap_frame(frame))
        })));

        let mut values = NodeValueRow::new();
        values.insert(
            "Source".into(),
            texture_value(filled_frame((2, 1), [0.1, 0.2, 0.3, 1.0])),
        );
        values.insert("gain".into(), NodeValue::Float(0.25));
        values.insert("Nothing".into(), NodeValue::None);
        // A null texture box is skipped by the type guard.
        values.insert("Null".into(), NodeValue::Texture(CHandle::null()));
        // A non-texture box in the texture channel logs and is skipped.
        values.insert(
            "Boxed".into(),
            NodeValue::Texture(oak_node::handle::make_owned(Job::FootageJob(
                FootageJobPayload::default(),
            ))),
        );
        let payload = PluginJobPayload {
            instance: PluginInstanceHandle(7),
            type_id: "org.oak.test-plugin".into(),
            time: Rational::new(1, 2),
            effect_input_id: String::new(),
            values,
        };

        let out = RenderEvalHooks::new()
            .process_plugin_job_value(&payload, 0, &mut HashSet::new())
            .expect("plugin value resolves");
        let NodeValue::Texture(handle) = &out else {
            panic!("expected a texture value, got {out:?}");
        };
        let texture = unsafe { oak_node::handle::get_checked::<Texture>(handle) }
            .cloned()
            .expect("resolved texture");
        assert_eq!(first_pixel(&texture), [0.5, 0.25, 0.125, 1.0]);
        set_plugin_executor(None);
    }

    #[test]
    fn color_transform_job_value_falls_back_to_its_input() {
        let payload = ColorTransformJobPayload {
            color_processor: Arc::new(oak_core::color::ColorProcessor::pass_through()),
            input: NodeValue::None,
            time: Rational::new(0, 1),
        };
        let out = RenderEvalHooks::new().process_color_transform_job_value(
            &payload,
            0,
            &mut HashSet::new(),
        );
        assert!(matches!(out, NodeValue::None));
    }

    #[test]
    fn composite_tracks_rejects_nonpositive_sizes() {
        assert!(composite_tracks(Vec::new(), (0, 4)).is_dummy());
        assert!(composite_tracks(Vec::new(), (4, -1)).is_dummy());
    }

    #[test]
    fn evaluate_block_frame_handles_missing_and_textureless_roots() {
        let mut graph = oak_node::graph::Graph::new();
        let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
        let seq = graph.add_node(score, sbehavior);
        let mut traverser = oak_node::traverser::Traverser::new();
        let mut hooks = RenderEvalHooks::new();

        // A stale root surfaces as `Failed` (the NotFound mapping).
        let missing = evaluate_block_frame(
            &graph,
            &mut traverser,
            &mut hooks,
            oak_node::id::NodeId::INVALID,
            Rational::new(0, 1),
        );
        assert!(matches!(missing, Err(Error::Failed(_))));

        // A root with no texture channel is `Ok(None)`.
        let none = evaluate_block_frame(
            &graph,
            &mut traverser,
            &mut hooks,
            seq,
            Rational::new(0, 1),
        );
        assert!(matches!(none, Ok(None)));
    }

    #[test]
    fn blend_transition_without_sides_is_inert() {
        let graph = oak_node::graph::Graph::new();
        let mut traverser = oak_node::traverser::Traverser::new();
        let mut hooks = RenderEvalHooks::new();
        let out = blend_transition(
            &graph,
            &mut traverser,
            &mut hooks,
            None,
            None,
            Rational::new(0, 1),
            0.5,
            "linear",
        );
        assert!(matches!(out, Ok(None)));
    }

    #[test]
    fn adjustment_chain_head_needs_an_effect_input() {
        let mut graph = oak_node::graph::Graph::new();
        let (core, behavior) = oak_node::track::TrackBehavior::create();
        let track = graph.add_node(core, behavior);
        assert!(adjustment_chain_head(&graph, track).is_none());
        assert!(adjustment_chain_head(&graph, oak_node::id::NodeId::INVALID).is_none());
    }

    #[test]
    fn layer_progress_clamps_and_reports_degenerate_spans() {
        assert_eq!(
            layer_progress(Rational::new(1, 1), Rational::new(1, 1), Rational::new(1, 1)),
            0.0
        );
        assert_eq!(
            layer_progress(Rational::new(2, 1), Rational::new(1, 1), Rational::new(1, 1)),
            0.0
        );
        assert_eq!(
            layer_progress(Rational::new(0, 1), Rational::new(2, 1), Rational::new(1, 1)),
            0.5
        );
        assert_eq!(
            layer_progress(Rational::new(0, 1), Rational::new(2, 1), Rational::new(9, 1)),
            1.0
        );
    }

    #[test]
    fn audio_layout_rejects_degenerate_and_oversized_ranges() {
        // Null denominator: the duration seconds stay 0.
        let params = audio_params(TimeRange::new(Rational::NULL, Rational::NULL));
        assert!(render_audio_samples(&params).is_err());

        // Longer than an hour.
        let params = audio_params(TimeRange::new(Rational::new(0, 1), Rational::new(7200, 1)));
        assert!(render_audio_samples(&params).is_err());
    }

    #[test]
    fn mix_audio_montage_skips_clips_outside_the_range() {
        let mut params = audio_params(TimeRange::new(Rational::new(0, 1), Rational::new(1, 48)));
        let clip = |in_: Rational, out: Rational| crate::ticket::MontageClip {
            filename: String::new(),
            stream_index: 0,
            in_time: in_,
            out_time: out,
            media_in: Rational::new(0, 1),
            gain: 1.0,
            effects: Vec::new(),
        };
        params.montage = vec![
            // No overlap at all.
            clip(Rational::new(2, 1), Rational::new(3, 1)),
            // Starts at the last sample (start_frame >= total_frames).
            clip(Rational::new(1999, 96000), Rational::new(1, 48)),
            // Rounds to zero frames.
            clip(Rational::new(1, 480000), Rational::new(4, 480000)),
        ];
        let mut acc = vec![0.0f32; 1000 * 2];
        mix_audio_montage(&params, 48000, 2, 1000, &mut acc).expect("skips");
        assert!(acc.iter().all(|&v| v == 0.0), "uncovered range stays silent");
    }

    #[test]
    fn scale_rgba_f32_bilinear_scales_and_clamps_alpha() {
        let src_w = 2i32;
        let src_h = 2i32;
        let src_stride = (src_w * 16) as usize;
        let mut src = vec![0u8; src_stride * src_h as usize];
        let put = |src: &mut [u8], x: usize, y: usize, rgba: [f32; 4]| {
            let off = y * src_stride + x * 16;
            for (i, v) in rgba.iter().enumerate() {
                src[off + i * 4..off + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
        };
        put(&mut src, 0, 0, [1.0, 0.0, 0.0, 3.0]);
        put(&mut src, 1, 0, [0.0, 1.0, 0.0, 3.0]);
        put(&mut src, 0, 1, [0.0, 0.0, 1.0, 3.0]);
        put(&mut src, 1, 1, [1.0, 1.0, 1.0, 3.0]);

        let dst_w = 4i32;
        let dst_h = 4i32;
        let dst_stride = (dst_w * 16) as usize;
        let mut dst = vec![0u8; dst_stride * dst_h as usize];
        scale_rgba_f32(
            src.as_ptr(),
            src_stride as i32,
            src_w,
            src_h,
            &mut dst,
            dst_stride as i32,
            dst_w,
            dst_h,
        );
        let read = |dst: &[u8], x: usize, y: usize| -> [f32; 4] {
            let off = y * dst_stride + x * 16;
            let mut out = [0f32; 4];
            for (i, v) in out.iter_mut().enumerate() {
                *v = f32::from_le_bytes(dst[off + i * 4..off + i * 4 + 4].try_into().unwrap());
            }
            out
        };

        // The top-left destination texel maps exactly onto the source
        // corner; alpha 3.0 clamps to 1.0.
        assert_eq!(read(&dst, 0, 0), [1.0, 0.0, 0.0, 1.0]);
        // The bottom-right texel lands on the far corner.
        assert_eq!(read(&dst, 3, 3), [1.0, 1.0, 1.0, 1.0]);
        // A mid texel interpolates bilinearly (0.25, 0.25 in source space).
        let mid = read(&dst, 1, 1);
        for (got, want) in mid.iter().zip([0.625f32, 0.25, 0.25, 1.0]) {
            assert!((got - want).abs() < 1e-5, "bilinear {mid:?}");
        }

        // Invalid geometry: nothing is written.
        let mut untouched = vec![0xAAu8; 16];
        scale_rgba_f32(
            src.as_ptr(),
            src_stride as i32,
            src_w,
            src_h,
            &mut untouched,
            16,
            0,
            1,
        );
        assert!(untouched.iter().all(|&b| b == 0xAA));
    }

    #[test]
    fn copy_rows_copies_strided_rows_and_rejects_bad_geometry() {
        let src: Vec<u8> = (0..64u16).map(|i| i as u8).collect();
        let mut dst = vec![0u8; 128];
        assert!(copy_rows(&mut dst, 64, &src, 32, 2, 2));
        assert_eq!(&dst[..32], &src[..32]);
        assert_eq!(&dst[64..96], &src[32..64]);
        assert!(dst[32..64].iter().all(|&b| b == 0), "gap untouched");
        assert!(dst[96..].iter().all(|&b| b == 0), "gap untouched");

        assert!(!copy_rows(&mut dst, 64, &src, 32, 0, 2));
        assert!(!copy_rows(&mut dst, 64, &src, 32, 2, 0));
        assert!(!copy_rows(&mut dst, 64, &src[..40], 32, 2, 2), "src too small");
        let mut small = vec![0u8; 64];
        assert!(!copy_rows(&mut small, 64, &src, 32, 2, 2), "dst too small");
    }

    /// A one-row F32 RGBA frame as raw bytes.
    fn f32_frame_bytes(size: (i32, i32), rgba: [f32; 4]) -> Vec<u8> {
        let mut frame = generate_frame(Rational::new(0, 1), size, PixelFormat::F32).unwrap();
        for px in frame.data.as_chunks_mut::<16>().0 {
            for (c, v) in px.as_chunks_mut::<4>().0.iter_mut().zip(rgba) {
                c.copy_from_slice(&v.to_le_bytes());
            }
        }
        frame.data
    }

    fn read_f32_px(data: &[u8], stride: usize, x: usize, y: usize) -> [f32; 4] {
        let off = y * stride + x * 16;
        let mut out = [0f32; 4];
        for (i, v) in out.iter_mut().enumerate() {
            *v = f32::from_le_bytes(data[off + i * 4..off + i * 4 + 4].try_into().unwrap());
        }
        out
    }

    fn adjustment_span(
        in_: i64,
        out: i64,
        track_index: usize,
        effects: Vec<crate::ticket::MontageEffect>,
    ) -> crate::ticket::AdjustmentSpan {
        crate::ticket::AdjustmentSpan {
            in_time: Rational::new(in_, 1),
            out_time: Rational::new(out, 1),
            track_index,
            effects,
        }
    }

    fn unknown_effect(type_id: &str) -> crate::ticket::MontageEffect {
        crate::ticket::MontageEffect {
            type_id: type_id.to_string(),
            enabled: true,
            effect_input_id: Some("Source".to_string()),
            params: Vec::new(),
        }
    }

    #[test]
    fn adjustment_span_opacity_scales_and_unknown_effects_stage() {
        let stride = 2 * 16;
        // Opacity-only: the fast in-place scale.
        let mut dst = f32_frame_bytes((2, 1), [0.8, 0.4, 0.2, 1.0]);
        let span = adjustment_span(0, 2, 0, vec![opacity_effect(true, 0.5)]);
        apply_adjustment_span(&mut dst, stride as i32, 2, 1, &span, Rational::new(0, 1));
        let px = read_f32_px(&dst, stride, 0, 0);
        assert!((px[0] - 0.4).abs() < 1e-6, "{px:?}");
        assert!((px[3] - 0.5).abs() < 1e-6, "{px:?}");

        // Unity opacity is skipped, an unknown effect forces the staged
        // path and passes the frame through (with the warn-once log).
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_instance_factory(Some(Arc::new(|_id: &str| None)));
        let mut dst = f32_frame_bytes((2, 1), [0.8, 0.4, 0.2, 1.0]);
        let span = adjustment_span(
            0,
            2,
            0,
            vec![
                opacity_effect(true, 1.0),
                unknown_effect("com.example.eval-unknown"),
            ],
        );
        apply_adjustment_span(&mut dst, stride as i32, 2, 1, &span, Rational::new(0, 1));
        assert_eq!(read_f32_px(&dst, stride, 0, 0), [0.8, 0.4, 0.2, 1.0]);
        set_plugin_instance_factory(None);

        // A span that does not cover the time is inert.
        let mut dst = f32_frame_bytes((2, 1), [0.8, 0.4, 0.2, 1.0]);
        let span = adjustment_span(5, 6, 0, vec![opacity_effect(true, 0.5)]);
        apply_adjustment_span(&mut dst, stride as i32, 2, 1, &span, Rational::new(0, 1));
        assert_eq!(read_f32_px(&dst, stride, 0, 0), [0.8, 0.4, 0.2, 1.0]);
    }

    #[test]
    fn montage_frame_into_rejects_bad_geometry_and_skips_uncovered_clips() {
        let params = crate::ticket::VideoTicketParams {
            viewer: 1,
            project: String::new(),
            time: Rational::new(0, 1),
            force_size: Some((2, 1)),
            force_format: Some(PixelFormat::F32),
            cache: None,
            cache_dir: None,
            cache_id: None,
            cache_timebase: None,
            footage: None,
            montage: vec![crate::ticket::MontageClip {
                filename: String::new(),
                stream_index: 0,
                in_time: Rational::new(1, 1),
                out_time: Rational::new(2, 1),
                media_in: Rational::new(0, 1),
                gain: 1.0,
                effects: Vec::new(),
            }],
            adjustments: Vec::new(),
        };

        // A short destination is rejected before anything is decoded.
        let mut tiny = [0u8; 8];
        assert!(render_montage_frame_into(
            Rational::new(0, 1),
            &params,
            (2, 1),
            &mut tiny,
            32
        )
        .is_err());

        // A non-positive size is rejected too.
        let mut dst = vec![0u8; 64];
        assert!(render_montage_frame_into(
            Rational::new(0, 1),
            &params,
            (0, 1),
            &mut dst,
            32
        )
        .is_err());

        // The clip does not cover t=0: the frame stays transparent black.
        let mut dst = vec![0xFFu8; 2 * 16];
        render_montage_frame_into(Rational::new(0, 1), &params, (2, 1), &mut dst, 32)
            .expect("uncovered clip is skipped");
        assert!(dst.iter().all(|&b| b == 0));
    }

    #[test]
    fn resolve_planar_footage_rejects_non_wgpu_contexts() {
        assert!(resolve_planar_footage(&Texture::dummy()).is_err());
        assert!(resolve_planar_footage(&planar_texture((2, 2))).is_err());
    }

    #[test]
    fn decoded_frame_cache_dedups_breaks_and_prunes_planar() {
        let key = |n: i32| -> DecodedFrameKey {
            (format!("frame{n}.mp4"), 0, (n as i64, 1), 2, 2)
        };

        // A duplicate key is a no-op.
        let mut cache = std::collections::HashMap::new();
        insert_cached_frame(&mut cache, key(0), Texture::dummy(), 1);
        insert_cached_frame(&mut cache, key(0), Texture::dummy(), 2);
        assert_eq!(cache.len(), 1);
        // Planar insertions take the planar-pruning path.
        insert_cached_frame(&mut cache, key(1), planar_texture((2, 2)), 3);
        assert_eq!(cache.len(), 2);

        // A cache full of tick-0 entries has no victim: the loop breaks.
        let mut cache = std::collections::HashMap::new();
        for i in 0..MAX_CACHED_FRAMES {
            cache.insert(key(i as i32), (Texture::dummy(), 0));
        }
        insert_cached_frame(&mut cache, key(100), Texture::dummy(), 7);
        assert_eq!(cache.len(), MAX_CACHED_FRAMES + 1);

        // Planar entries are pruned to the keep budget, LRU first.
        let mut cache = std::collections::HashMap::new();
        for i in 0..(MAX_CACHED_PLANAR_FRAMES + 3) {
            cache.insert(key(i as i32), (planar_texture((2, 2)), i as u64 + 1));
        }
        prune_planar_frames(&mut cache, 2);
        assert_eq!(
            cache.values().filter(|(t, _)| t.is_planar()).count(),
            2,
            "planar entries pruned to the budget"
        );
        // The oldest planar entries went first.
        assert!(cache.contains_key(&key(MAX_CACHED_PLANAR_FRAMES as i32 + 2)));
        // At or under the budget: a no-op.
        prune_planar_frames(&mut cache, 99);
        assert_eq!(cache.values().filter(|(t, _)| t.is_planar()).count(), 2);
    }

    #[test]
    fn eviction_victim_falls_back_to_software_sessions() {
        type DecoderMap =
            std::collections::HashMap<(String, i32), (Arc<dyn oak_codec::decoder::Decoder>, u64)>;
        let mut cache: DecoderMap = std::collections::HashMap::new();
        cache.insert(
            ("a.mp4".to_string(), 0),
            (Arc::new(FFmpegDecoder::new()), 5),
        );
        cache.insert(
            ("b.mp4".to_string(), 0),
            (Arc::new(FFmpegDecoder::new()), 3),
        );
        assert_eq!(
            eviction_victim(&cache),
            Some(("b.mp4".to_string(), 0)),
            "no hardware session: the LRU software one goes"
        );
        assert_eq!(eviction_victim(&DecoderMap::new()), None);
    }

    #[test]
    fn missing_colorimetry_warning_is_callable_more_than_once() {
        warn_missing_colorimetry_once();
        warn_missing_colorimetry_once();
    }

    #[test]
    fn opacity_factor_and_channel_scaling_edge_cases() {
        assert!(
            opacity_factor(&unknown_effect("com.example.opacity-test")).is_none(),
            "a non-opacity effect has no factor"
        );
        assert!(opacity_factor(&opacity_effect(true, 1.0)).is_none(), "unity is skipped");
        assert_eq!(opacity_factor(&opacity_effect(true, 0.5)), Some(0.5));
        assert_eq!(
            opacity_factor(&opacity_effect(true, 2.0)),
            Some(2.0),
            "values above one scale up"
        );
        // An Opacity effect without the value input defaults to unity.
        let mut no_param = opacity_effect(true, 0.5);
        no_param.params.clear();
        assert!(opacity_factor(&no_param).is_none());

        let mut bytes = [0u8; 32];
        scale_channels_in_place(&mut bytes, 0, 2, 1, 2.0);
        scale_channels_in_place(&mut bytes, 32, 0, 1, 2.0);
        scale_channels_in_place(&mut bytes, 32, 2, 0, 2.0);
        assert_eq!(bytes, [0u8; 32], "invalid geometry is inert");
    }

    #[test]
    fn montage_opacity_on_non_cpu_textures_warns_and_passes_through() {
        let effect = opacity_effect(true, 0.5);
        let out = apply_montage_effect(planar_texture((2, 1)), &effect, Rational::new(0, 1));
        assert!(out.is_planar(), "a planar texture is returned unchanged");
        let gpu = Texture::gpu(Arc::new(UnusedCtx), 1, 2, 1, PixelFormat::F32);
        let out = apply_montage_effect(gpu, &effect, Rational::new(0, 1));
        assert!(matches!(out, Texture::Gpu { .. }));
    }

    #[test]
    fn montage_effect_without_a_resolvable_evaluator_passes_through() {
        let clip = clip_with_effects(vec![unknown_effect("com.example.no-evaluator")]);
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        crate::ofxhost::install_client(None);
        set_plugin_instance_factory(Some(Arc::new(|_id: &str| None)));
        let out = apply_clip_effects(
            solid_texture(0.8, 0.4, 0.2, 1.0),
            &clip,
            Rational::new(0, 1),
        );
        assert_eq!(first_pixel(&out), [0.8, 0.4, 0.2, 1.0]);
        set_plugin_instance_factory(None);
    }

    #[test]
    fn produced_frame_non_f32_uses_the_cpu_generator() {
        for format in [PixelFormat::U8, PixelFormat::U16] {
            let params = crate::ticket::VideoTicketParams {
                viewer: 1,
                project: String::new(),
                time: Rational::new(0, 1),
                force_size: Some((3, 2)),
                force_format: Some(format),
                cache: None,
                cache_dir: None,
                cache_id: None,
                cache_timebase: None,
                footage: None,
                montage: Vec::new(),
                adjustments: Vec::new(),
            };
            let tex = render_produced_frame(Rational::new(1, 1), &params).unwrap();
            assert!(matches!(tex, Texture::Cpu(_)), "{format:?} uses the CPU producer");
            assert_eq!(tex.size(), (3, 2));
            assert_eq!(tex.format(), format);
        }
    }

    #[test]
    fn generate_frame_with_an_invalid_format_reports_nomem() {
        assert!(generate_frame(
            Rational::new(0, 1),
            (4, 4),
            PixelFormat::Invalid
        )
        .is_err());
    }

    #[test]
    fn convert_decoded_to_working_handles_missing_metadata_and_short_buffers() {
        let _guard = working_space_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = oak_core::color::pipeline_working_space();
        let output = oak_core::color::pipeline_output_spec();
        oak_core::color::set_pipeline_color_settings(
            oak_core::colormath::WorkingColorSpace::AcesCg,
            output,
        );
        let mut decoded = oak_codec::frame::Frame::new();
        decoded.params = None; // no colorimetry metadata

        // The generic sRGB fallback converts in place.
        let mut dst = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        convert_decoded_to_working(&mut dst, &decoded);

        // Zero-sized destinations return before touching the buffer.
        let mut zero = Frame::new();
        convert_decoded_to_working(&mut zero, &decoded);

        // A short buffer breaks out of the row loop.
        let mut short = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        short.data.truncate(16);
        convert_decoded_to_working(&mut short, &decoded);

        oak_core::color::set_pipeline_color_settings(previous, output);
    }

    #[test]
    fn footage_working_lut_caches_and_clears() {
        let _guard = working_space_test_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = oak_core::color::pipeline_working_space();
        let output = oak_core::color::pipeline_output_spec();
        oak_core::color::set_pipeline_color_settings(
            oak_core::colormath::WorkingColorSpace::AcesCg,
            output,
        );

        let first = footage_working_lut(1, 1);
        assert!(first.is_some());
        let second = footage_working_lut(1, 1);
        assert_eq!(
            first.expect("first LUT").0,
            second.expect("cached LUT").0,
            "the second request hits the cache"
        );
        // 16+ distinct colorimetries flush the per-process cache.
        for i in 0..20i32 {
            let _ = footage_working_lut(10 + i * 3, 20 + i * 7);
        }

        oak_core::color::set_pipeline_color_settings(previous, output);
    }

    #[test]
    fn color_transform_lut_cache_hits_and_clears() {
        let Some(processor) = red_doubling_processor("lutcache") else {
            return;
        };
        assert!(color_transform_lut(&processor).is_some());
        assert!(
            color_transform_lut(&processor).is_some(),
            "the second request hits the processor's cache entry"
        );

        // Distinct processors flush the cache once it holds 16 entries.
        let mut ids = std::collections::HashSet::new();
        for i in 1..20 {
            if let Some(p) = lut_processor(&format!("lutflush{i}"), 1.0 + i as f32 * 0.25) {
                ids.insert(p.cache_id());
                let _ = color_transform_lut(&p);
            }
        }
        if ids.len() < 2 {
            eprintln!("OCIO cache ids are not distinct; cache flush not exercised");
        }
    }

    /// Serializes the tests that set process-wide environment variables
    /// (`OAK_PERF`) or the decode-service slot.
    static ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Saves `OAK_PERF` and restores its previous value (or absence) on
    /// drop, so a panicking assertion cannot leak the perf override into
    /// the next serialized env test. Callers hold [`ENV_TEST_LOCK`].
    struct PerfEnvGuard(Option<std::ffi::OsString>);

    impl PerfEnvGuard {
        fn enable() -> PerfEnvGuard {
            let previous = std::env::var_os("OAK_PERF");
            std::env::set_var("OAK_PERF", "1");
            PerfEnvGuard(previous)
        }
    }

    impl Drop for PerfEnvGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("OAK_PERF", value),
                None => std::env::remove_var("OAK_PERF"),
            }
        }
    }

    /// Serializes the tests that flip [`GPU_COMPOSITE_FAILED`].
    static COMPOSITE_FLAG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A footage node probed from `path`, wired to a clip block spanning
    /// `[in_, out)`, on `graph`.
    fn add_footage_clip(
        graph: &mut oak_node::graph::Graph,
        path: &str,
        in_: Rational,
        out: Rational,
    ) -> oak_node::id::NodeId {
        let mut footage = oak_node::footage::FootageBehavior::new(path);
        footage.probe().expect("probe the test clip");
        let footage = graph.add_node(oak_node::node::NodeCore::new(), Box::new(footage));
        let (ccore, cbehavior) = oak_node::block::clip_create();
        let clip = graph.add_node(ccore, cbehavior);
        graph
            .connect(
                footage,
                clip,
                oak_node::block::clip_input::TEXTURE_INPUT,
                -1,
            )
            .expect("footage -> clip");
        graph
            .get_mut(clip)
            .unwrap()
            .behavior
            .as_any_mut()
            .unwrap()
            .downcast_mut::<oak_node::block::ClipBlockBehavior>()
            .expect("clip block")
            .core
            .range = TimeRange::new(in_, out);
        clip
    }

    /// An enabled clip block of `[in_, out)` with nothing connected.
    fn add_bare_clip(graph: &mut oak_node::graph::Graph, in_: Rational, out: Rational) -> oak_node::id::NodeId {
        let (ccore, cbehavior) = oak_node::block::clip_create();
        let clip = graph.add_node(ccore, cbehavior);
        graph
            .get_mut(clip)
            .unwrap()
            .behavior
            .as_any_mut()
            .unwrap()
            .downcast_mut::<oak_node::block::ClipBlockBehavior>()
            .expect("clip block")
            .core
            .range = TimeRange::new(in_, out);
        clip
    }

    /// One sequence with three video tracks: V0 (a plain footage clip),
    /// V1 (two clips joined by a transition covering t=1) and V2 (an
    /// adjustment block with an opacity chain). Rendering t=1 walks the
    /// clip, transition and adjustment steps.
    fn build_three_step_project(path: &str) -> (Arc<Mutex<Project>>, oak_node::id::NodeId) {
        let project = Project::new();
        let seq;
        {
            let mut p = project.lock().unwrap();
            let graph = &mut p.graph;
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = graph.add_node(score, sbehavior);
            let (tlcore, tlbehavior) = oak_node::track::TrackListBehavior::create();
            let tl = graph.add_node(tlcore, tlbehavior);

            // V0: one clip covering t=1.
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let v0 = graph.add_node(tcore, tbehavior);
            let c0 = add_footage_clip(graph, path, Rational::new(0, 1), Rational::new(2, 1));
            graph
                .get_mut(v0)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackBehavior>()
                .unwrap()
                .append_block(c0);

            // V1: two clips joined by a transition covering t=1.
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let v1 = graph.add_node(tcore, tbehavior);
            let a = add_footage_clip(graph, path, Rational::new(0, 1), Rational::new(1, 1));
            let b = add_footage_clip(graph, path, Rational::new(1, 1), Rational::new(2, 1));
            let (trcore, trbehavior) = oak_node::block::transition_create();
            let transition = graph.add_node(trcore, trbehavior);
            {
                let t = graph
                    .get_mut(transition)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::block::TransitionBlockBehavior>()
                    .unwrap();
                t.core.range = TimeRange::new(Rational::new(1, 2), Rational::new(3, 2));
                t.core.enabled = true;
            }
            graph
                .connect(
                    a,
                    transition,
                    oak_node::block::transition_input::OUT_BLOCK,
                    -1,
                )
                .unwrap();
            graph
                .connect(
                    b,
                    transition,
                    oak_node::block::transition_input::IN_BLOCK,
                    -1,
                )
                .unwrap();
            {
                let track = graph
                    .get_mut(v1)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::track::TrackBehavior>()
                    .unwrap();
                track.append_block(a);
                track.append_block(transition);
                track.append_block(b);
            }

            // V2: an adjustment block with an opacity chain on top.
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let v2 = graph.add_node(tcore, tbehavior);
            let (acore, abehavior) = oak_node::block::adjustment_create();
            let adjustment = graph.add_node(acore, abehavior);
            {
                let a = graph
                    .get_mut(adjustment)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::block::AdjustmentBlockBehavior>()
                    .unwrap();
                a.core.range = TimeRange::new(Rational::new(0, 1), Rational::new(2, 1));
                a.core.enabled = true;
            }
            let (ecore, ebehavior) = oak_node::nodes::opacity::create();
            let effect = graph.add_node(ecore, ebehavior);
            graph
                .connect(
                    effect,
                    adjustment,
                    oak_node::block::adjustment_input::TEXTURE_INPUT,
                    -1,
                )
                .unwrap();
            graph.get_mut(effect).unwrap().core.set_standard_value(
                oak_node::nodes::opacity::VALUE_INPUT,
                -1,
                NodeValue::Float(0.5),
            );
            graph
                .get_mut(v2)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackBehavior>()
                .unwrap()
                .append_block(adjustment);

            let tl_behavior = graph
                .get_mut(tl)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackListBehavior>()
                .unwrap();
            tl_behavior.tracks.push(v0);
            tl_behavior.tracks.push(v1);
            tl_behavior.tracks.push(v2);
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(tl);
        }
        (project, seq)
    }

    /// A sequence whose track/track-block lists mix stale ids, foreign
    /// behaviors and degenerate blocks; returns the sequence plus the
    /// bare and undecodable clips for direct evaluation.
    fn build_degenerate_project(
        missing_media: &str,
    ) -> (
        Arc<Mutex<Project>>,
        oak_node::id::NodeId,
        oak_node::id::NodeId,
        oak_node::id::NodeId,
    ) {
        use oak_node::id::NodeId;
        let project = Project::new();
        let empty_clip;
        let bad_clip;
        let seq;
        {
            let mut p = project.lock().unwrap();
            let graph = &mut p.graph;
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = graph.add_node(score, sbehavior);
            let (tlcore, tlbehavior) = oak_node::track::TrackListBehavior::create();
            let tl = graph.add_node(tlcore, tlbehavior);

            // Stale and foreign entries in the sequence's track lists.
            let stale_list = NodeId::from_identity(900_001).unwrap();
            let (score2, sbehavior2) = oak_node::sequence::SequenceBehavior::create();
            let not_a_tracklist = graph.add_node(score2, sbehavior2);

            // A real list with stale/foreign entries in its track list.
            let stale_track = NodeId::from_identity(900_002).unwrap();
            let (score3, sbehavior3) = oak_node::sequence::SequenceBehavior::create();
            let not_a_track = graph.add_node(score3, sbehavior3);
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let track = graph.add_node(tcore, tbehavior);

            // Blocks: a stale id, a transition with no neighbors and a
            // bare clip.
            let stale_block = NodeId::from_identity(900_003).unwrap();
            let (trcore, trbehavior) = oak_node::block::transition_create();
            let transition = graph.add_node(trcore, trbehavior);
            {
                let t = graph
                    .get_mut(transition)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::block::TransitionBlockBehavior>()
                    .unwrap();
                t.core.range = TimeRange::new(Rational::new(0, 1), Rational::new(2, 1));
                t.core.enabled = true;
            }
            empty_clip = add_bare_clip(graph, Rational::new(0, 1), Rational::new(2, 1));
            {
                let track_behavior = graph
                    .get_mut(track)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::track::TrackBehavior>()
                    .unwrap();
                track_behavior.blocks.push(stale_block);
                track_behavior.blocks.push(transition);
                track_behavior.blocks.push(empty_clip);
            }

            // A second real track whose clip references a missing file:
            // the unresolved footage-job box is left in the table.
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let track2 = graph.add_node(tcore, tbehavior);
            bad_clip = add_footage_clip(
                graph,
                missing_media,
                Rational::new(0, 1),
                Rational::new(2, 1),
            );
            graph
                .get_mut(track2)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackBehavior>()
                .unwrap()
                .append_block(bad_clip);

            {
                let tl_behavior = graph
                    .get_mut(tl)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::track::TrackListBehavior>()
                    .unwrap();
                tl_behavior.tracks.push(stale_track);
                tl_behavior.tracks.push(not_a_track);
                tl_behavior.tracks.push(track);
                tl_behavior.tracks.push(track2);
            }
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(stale_list);
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(not_a_tracklist);
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(tl);
        }
        (project, seq, empty_clip, bad_clip)
    }

    #[test]
    fn render_graph_frame_rejects_bad_format_and_size() {
        let project = Project::new();
        let seq;
        {
            let mut p = project.lock().unwrap();
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = p.graph.add_node(score, sbehavior);
        }
        assert!(render_graph_frame(
            &project,
            seq,
            Rational::new(0, 1),
            (16, 16),
            PixelFormat::U8
        )
        .is_err());
        assert!(render_graph_frame(
            &project,
            seq,
            Rational::new(0, 1),
            (0, 16),
            PixelFormat::F32
        )
        .is_err());
    }

    #[test]
    fn render_graph_frame_skips_stale_and_textureless_steps() {
        // Probe a real clip, then delete the file: the footage job is built
        // at evaluation time but its decode fails, so the unresolved box
        // reaches the texture-channel checks.
        let path = std::env::temp_dir().join(format!(
            "oakrender_degenerate_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        let name = path.to_string_lossy().into_owned();
        let (project, seq, empty_clip, bad_clip) = build_degenerate_project(&name);
        let _ = std::fs::remove_file(&path);

        let out = render_graph_frame(
            &project,
            seq,
            Rational::new(1, 2),
            (4, 4),
            PixelFormat::F32,
        );
        assert_eq!(out.expect("degenerate render").size(), (4, 4));

        // Directly: a clip with nothing connected yields no texture, an
        // undecodable footage job leaves its box behind.
        let mut traverser = oak_node::traverser::Traverser::new();
        let mut hooks = RenderEvalHooks::new();
        {
            let guard = project.lock().unwrap();
            let empty = evaluate_block_frame(
                &guard.graph,
                &mut traverser,
                &mut hooks,
                empty_clip,
                Rational::new(1, 2),
            );
            assert!(matches!(empty, Ok(None)), "a bare clip produces nothing");
            let bad = evaluate_block_frame(
                &guard.graph,
                &mut traverser,
                &mut hooks,
                bad_clip,
                Rational::new(1, 2),
            );
            assert!(
                matches!(bad, Ok(None)),
                "an unresolved footage job is not a texture"
            );
        }
    }

    #[test]
    fn render_graph_frame_stamps_cpu_frames_when_the_gpu_composite_is_off() {
        let project = Project::new();
        let seq;
        {
            let mut p = project.lock().unwrap();
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = p.graph.add_node(score, sbehavior);
        }
        let _guard = COMPOSITE_FLAG_LOCK.lock().unwrap();
        let previous =
            GPU_COMPOSITE_FAILED.swap(true, std::sync::atomic::Ordering::Relaxed);
        let out = render_graph_frame(
            &project,
            seq,
            Rational::new(3, 1),
            (4, 4),
            PixelFormat::F32,
        );
        GPU_COMPOSITE_FAILED.store(previous, std::sync::atomic::Ordering::Relaxed);
        match out.expect("cpu composite") {
            Texture::Cpu(frame) => {
                assert_eq!(frame.timestamp, Rational::new(3, 1));
                assert_eq!((frame.width, frame.height), (4, 4));
            }
            Texture::Gpu { .. } | Texture::Planar(_) => {
                panic!("the CPU fallback must produce a CPU frame")
            }
        }
    }

    #[test]
    fn oak_perf_graph_render_logs_every_step() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_perf_graph_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        let (project, seq) = build_three_step_project(&path.to_string_lossy());
        let _perf = PerfEnvGuard::enable();
        let out = render_graph_frame(
            &project,
            seq,
            Rational::new(1, 1),
            (16, 16),
            PixelFormat::F32,
        );
        assert_eq!(out.expect("perf render").size(), (16, 16));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn oak_perf_footage_decode_logs_and_reuses_sessions() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_perf_decode_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        let name = path.to_string_lossy().into_owned();
        let _perf = PerfEnvGuard::enable();
        let first = open_decoder(&name, 0);
        let second = open_decoder(&name, 0);
        let decoded = render_footage_frame(&name, 0, Rational::new(0, 1), (16, 16), PixelFormat::F32);
        assert!(first.is_ok(), "the first open logs (request)");
        assert!(second.is_ok(), "the second open logs CACHED");
        assert!(decoded.is_ok(), "the decode logs [decode]");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn shut_down_decode_service_falls_back_to_inline_decode() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_stopped_service_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        let name = path.to_string_lossy().into_owned();
        let service = crate::pipeline::DecodeService::new(1, Arc::new(|| true));
        service.shutdown();
        crate::pipeline::install_decode_service(Some(service));
        let direct = render_footage_frame(&name, 0, Rational::new(0, 1), (16, 16), PixelFormat::F32);
        let staged =
            render_footage_frame_staged(&name, 0, Rational::new(0, 1), (16, 16), PixelFormat::F32);
        crate::pipeline::install_decode_service(None);
        assert!(direct.is_ok(), "a gone service falls back inline");
        assert!(staged.is_ok(), "the staged path falls back inline too");
        let _ = std::fs::remove_file(&path);
    }

    // ---- Coverage edges: GPU shader jobs and composites -----------------

    /// A payload that asks a registered node for a shader variant nobody
    /// implements: the job warns and yields nothing.
    #[test]
    fn gpu_shader_job_reports_missing_shaders_and_bad_bindings() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }

        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.checkerboard".into(),
            shader_id: "no-such-shader".into(),
            effect_input: "tex_in".into(),
            ..ShaderJobPayload::default()
        };
        assert!(
            RenderEvalHooks::new().process_shader_job(&payload).is_none(),
            "an unknown shader id yields no texture"
        );

        // merge (no declared effect input): a real base plus a null handle
        // and an unresolved planar texture. Both bad inputs are skipped,
        // the pass still runs at the base's size.
        let mut params = NodeValueRow::new();
        params.insert(
            "base_in".into(),
            texture_value(filled_frame((4, 4), [1.0, 0.0, 0.0, 1.0])),
        );
        params.insert("blend_in".into(), NodeValue::Texture(CHandle::null()));
        params.insert(
            "extra_in".into(),
            NodeValue::Texture(oak_node::handle::make_owned(planar_texture((4, 4)))),
        );
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.merge".into(),
            shader_id: String::new(),
            effect_input: String::new(),
            params,
            ..ShaderJobPayload::default()
        };
        let out = RenderEvalHooks::new()
            .process_shader_job(&payload)
            .expect("the merge runs with the skippable inputs unbound");
        assert_eq!(out.size(), (4, 4));
    }

    #[test]
    fn gpu_shader_job_binds_nested_shader_payloads() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let nested = Job::ShaderJob(ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
            params: NodeValueRow::from([(
                "color_in".to_string(),
                NodeValue::Color([0.0, 1.0, 0.0, 1.0]),
            )]),
            ..ShaderJobPayload::default()
        });
        let mut params = NodeValueRow::new();
        params.insert(
            "base_in".into(),
            texture_value(filled_frame((4, 4), [1.0, 0.0, 0.0, 1.0])),
        );
        params.insert(
            "blend_in".into(),
            NodeValue::Texture(oak_node::handle::make_owned(nested)),
        );
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.merge".into(),
            shader_id: String::new(),
            effect_input: String::new(),
            params,
            ..ShaderJobPayload::default()
        };
        let out = RenderEvalHooks::new()
            .process_shader_job(&payload)
            .expect("the nested generator resolves through the bind");
        let px = first_pixel(&out);
        assert!(
            px[1] > 0.9 && px[0] < 0.1,
            "the generated green covers the red base: {px:?}"
        );
    }

    #[test]
    fn gpu_grading_job_splices_the_ocio_grading_stub() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        if oak_core::color::set_up_default_config().is_err() {
            eprintln!("bundled OCIO missing; skipping");
            return;
        }
        if oak_core::color::grading_primary_function_shader(oak_core::color::GradingStyle::Lin)
            .is_none()
        {
            eprintln!("no OCIO grading stub; skipping");
            return;
        }
        let mut params = NodeValueRow::new();
        params.insert(
            "tex_in".into(),
            texture_value(filled_frame((4, 4), [0.5, 0.5, 0.5, 1.0])),
        );
        params.insert(
            "OCIO_NAMESPACE_grading_primary_brightness".into(),
            NodeValue::Float(1.0),
        );
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.ociogradingtransformlinear".into(),
            shader_id: String::new(),
            effect_input: "tex_in".into(),
            params,
            ..ShaderJobPayload::default()
        };
        let out = RenderEvalHooks::new().process_shader_job(&payload);
        assert!(
            out.is_some(),
            "the grading node renders with the spliced OCIO stub"
        );
    }

    #[test]
    fn gpu_composite_reads_foreign_textures_and_rejects_planar() {
        let Some(ctx) = oak_core::backend::shared_gpu_or_skip("an eval GPU test") else {
            return;
        };
        assert!(composite_tracks_gpu(&ctx, &[], (0, 1)).is_err());

        // A GPU texture from another context is read back and re-uploaded.
        let foreign = echo_gpu_texture((2, 1), [0.25, 0.5, 0.75, 1.0], 0xDEAD_BEEF);
        let out = composite_tracks_gpu(&ctx, &[foreign], (2, 1)).expect("foreign readback");
        let px = first_pixel(&out);
        assert!(
            (px[0] - 0.25).abs() < 5e-3
                && (px[1] - 0.5).abs() < 5e-3
                && (px[2] - 0.75).abs() < 5e-3,
            "foreign texture survives the readback: {px:?}"
        );

        // An unresolved planar frame is a hard error; the accumulator is
        // torn down on the way out.
        let planar = planar_texture((2, 1));
        assert!(composite_tracks_gpu(&ctx, &[planar], (2, 1)).is_err());
    }

    #[test]
    fn composite_tracks_cpu_fallback_skips_unreadable_and_mismatched_frames() {
        let _guard = COMPOSITE_FLAG_LOCK.lock().unwrap();
        let previous = GPU_COMPOSITE_FAILED.swap(false, std::sync::atomic::Ordering::Relaxed);
        // The planar frame fails the GPU pass (and flips the sticky flag);
        // the CPU fallback then skips it, skips a wrongly-sized frame and
        // composites the valid one.
        let planar = planar_texture((2, 1));
        let wrong = filled_frame((3, 1), [0.0, 1.0, 0.0, 1.0]);
        let right = filled_frame((2, 1), [1.0, 0.0, 0.0, 1.0]);
        let out = composite_tracks(vec![planar, wrong, right], (2, 1));
        GPU_COMPOSITE_FAILED.store(previous, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(out.size(), (2, 1));
        let px = first_pixel(&out);
        assert!(
            (px[0] - 1.0).abs() < 1e-4 && px[1].abs() < 1e-4 && (px[3] - 1.0).abs() < 1e-4,
            "only the valid frame composites: {px:?}"
        );
    }

    // ---- Coverage edges: montage clip effects with GPU results ----------

    fn montage_params_with_effect(
        path: &str,
        effect_type_id: &str,
    ) -> crate::ticket::VideoTicketParams {
        crate::ticket::VideoTicketParams {
            viewer: 1,
            project: String::new(),
            time: Rational::new(0, 1),
            force_size: Some((16, 16)),
            force_format: Some(PixelFormat::F32),
            cache: None,
            cache_dir: None,
            cache_id: None,
            cache_timebase: None,
            footage: None,
            montage: vec![crate::ticket::MontageClip {
                filename: path.to_string(),
                stream_index: 0,
                in_time: Rational::new(0, 1),
                out_time: Rational::new(2, 1),
                media_in: Rational::new(0, 1),
                gain: 1.0,
                effects: vec![unknown_effect(effect_type_id)],
            }],
            adjustments: Vec::new(),
        }
    }

    #[test]
    fn montage_reads_back_gpu_textures_from_clip_effects() {
        let _env = ENV_TEST_LOCK.lock().unwrap();
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_montage_gpu_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        crate::ofxhost::install_client(None);
        set_plugin_instance_factory(Some(Arc::new(|_id: &str| Some(7))));
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            let frame = filled_frame(req.src.size(), [0.5, 0.5, 0.5, 1.0]).to_frame()?;
            Ok(Texture::gpu(
                Arc::new(FrameEchoCtx { frame }),
                0xE0,
                req.src.size().0,
                req.src.size().1,
                PixelFormat::F32,
            ))
        })));
        let params = montage_params_with_effect(&path.to_string_lossy(), "com.example.gpu-effect");
        let mut dst = vec![0u8; 16 * 16 * 16];
        render_montage_frame_into(Rational::new(0, 1), &params, (16, 16), &mut dst, 256)
            .expect("the GPU effect result is read back and composited");
        let px = read_f32_px(&dst, 256, 1, 1);
        assert!((px[0] - 0.5).abs() < 1e-4, "gpu effect pixels land: {px:?}");
        set_plugin_executor(None);
        set_plugin_instance_factory(None);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn montage_rejects_planar_textures_from_clip_effects() {
        let _env = ENV_TEST_LOCK.lock().unwrap();
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_montage_planar_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        crate::ofxhost::install_client(None);
        set_plugin_instance_factory(Some(Arc::new(|_id: &str| Some(7))));
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            Ok(planar_texture(req.src.size()))
        })));
        let params =
            montage_params_with_effect(&path.to_string_lossy(), "com.example.planar-effect");
        let mut dst = vec![0u8; 16 * 16 * 16];
        let err = render_montage_frame_into(Rational::new(0, 1), &params, (16, 16), &mut dst, 256);
        set_plugin_executor(None);
        set_plugin_instance_factory(None);
        let _ = std::fs::remove_file(&path);
        assert!(err.is_err(), "an unresolved planar texture is rejected");
    }

    /// The single OFX host client owns dispatch when one is installed:
    /// the montage effect path takes the host branch, and a host that
    /// cannot come up yields the purple failure frame.
    #[test]
    #[cfg(unix)]
    fn montage_effect_uses_the_installed_ofx_host() {
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        let host = crate::ofxhost::OfxHost::new(crate::ofxhost::OfxHostConfig {
            host_bin: Some(std::path::PathBuf::from("/bin/false")),
            max_failures: 1,
            ..Default::default()
        })
        .unwrap();
        crate::ofxhost::install_client(Some(host));
        let out = apply_montage_effect(
            solid_texture(0.8, 0.4, 0.2, 1.0),
            &unknown_effect("com.example.host-effect"),
            Rational::new(0, 1),
        );
        crate::ofxhost::install_client(None);
        assert_eq!(
            first_pixel(&out),
            [1.0, 0.0, 1.0, 1.0],
            "a failed host render paints the purple frame"
        );
    }

    /// A test-only node behavior that emits a null texture handle, so the
    /// graph seams' null-handle guards are reachable without a producer
    /// bug.
    struct NullTextureBehavior;

    impl oak_node::node::NodeBehavior for NullTextureBehavior {
        fn name(&self) -> &str {
            "NullTexture"
        }
        fn type_id(&self) -> &str {
            "org.oak.test.nulltexture"
        }
        fn duplicate(
            &self,
            _core: &oak_node::node::NodeCore,
        ) -> Option<Box<dyn oak_node::node::NodeBehavior>> {
            Some(Box::new(Self))
        }
        fn value(
            &self,
            _core: &oak_node::node::NodeCore,
            _inputs: &NodeValueRow,
            _time: Rational,
            table: &mut NodeValueTable,
        ) {
            table.push(
                oak_node::value::ValueType::Texture,
                NodeValue::Texture(CHandle::null()),
                None,
            );
        }
    }

    #[test]
    fn null_texture_outputs_are_skipped_by_the_graph_paths() {
        let project = Project::new();
        let seq;
        let clip;
        {
            let mut p = project.lock().unwrap();
            let graph = &mut p.graph;
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = graph.add_node(score, sbehavior);
            let (tlcore, tlbehavior) = oak_node::track::TrackListBehavior::create();
            let tl = graph.add_node(tlcore, tlbehavior);
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let track = graph.add_node(tcore, tbehavior);

            let null_node = graph.add_node(
                oak_node::node::NodeCore::new(),
                Box::new(NullTextureBehavior),
            );
            clip = add_bare_clip(graph, Rational::new(0, 1), Rational::new(1, 1));
            graph
                .connect(
                    null_node,
                    clip,
                    oak_node::block::clip_input::TEXTURE_INPUT,
                    -1,
                )
                .expect("null node -> clip");
            graph
                .get_mut(track)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackBehavior>()
                .unwrap()
                .append_block(clip);
            graph
                .get_mut(tl)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackListBehavior>()
                .unwrap()
                .tracks
                .push(track);
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(tl);
        }

        // Direct: a null handle is not a texture (`Ok(None)`).
        {
            let guard = project.lock().unwrap();
            let mut traverser = oak_node::traverser::Traverser::new();
            let mut hooks = RenderEvalHooks::new();
            let out = evaluate_block_frame(
                &guard.graph,
                &mut traverser,
                &mut hooks,
                clip,
                Rational::new(1, 2),
            );
            assert!(matches!(out, Ok(None)));
        }

        // Render: the clip's null texture is skipped and the frame stays
        // transparent black.
        let frame = render_graph_frame(
            &project,
            seq,
            Rational::new(1, 2),
            (4, 4),
            PixelFormat::F32,
        )
        .expect("null texture render");
        assert_eq!(frame.size(), (4, 4));
        let readback = frame.to_frame().expect("readback");
        assert!(readback.data.iter().all(|&b| b == 0));
    }

    // ---- Coverage edges: shader bind failures and adjustment sweeps ----

    /// A behavior with a synthetic texture output, so the transition and
    /// sweep seams can be driven without a real producer. `value` is
    /// pushed on the texture channel (nothing when `None`).
    struct FixedTextureBehavior {
        value: Option<NodeValue>,
    }

    impl oak_node::node::NodeBehavior for FixedTextureBehavior {
        fn name(&self) -> &str {
            "FixedTexture"
        }
        fn type_id(&self) -> &str {
            "org.oak.test.fixedtexture"
        }
        fn duplicate(
            &self,
            _core: &oak_node::node::NodeCore,
        ) -> Option<Box<dyn oak_node::node::NodeBehavior>> {
            Some(Box::new(Self {
                value: self.value.clone(),
            }))
        }
        fn value(
            &self,
            _core: &oak_node::node::NodeCore,
            _inputs: &NodeValueRow,
            _time: Rational,
            table: &mut NodeValueTable,
        ) {
            if let Some(value) = &self.value {
                table.push(oak_node::value::ValueType::Texture, value.clone(), None);
            }
        }
    }

    /// A behavior whose fragment source cannot compile (the shader-job
    /// path must warn and report no texture).
    struct BadShaderBehavior;

    impl oak_node::node::NodeBehavior for BadShaderBehavior {
        fn name(&self) -> &str {
            "BadShader"
        }
        fn type_id(&self) -> &str {
            "org.oak.test.badshader"
        }
        fn duplicate(
            &self,
            _core: &oak_node::node::NodeCore,
        ) -> Option<Box<dyn oak_node::node::NodeBehavior>> {
            Some(Box::new(Self))
        }
        fn shader_code(&self, _request: &str) -> Option<String> {
            Some("%%% this is not valid glsl %%%".to_string())
        }
    }

    /// A core with an effect input `tex_in`, optionally not connectable
    /// (the sweep's refused-connection error path).
    fn effect_head_core(connectable: bool) -> oak_node::node::NodeCore {
        let mut core = oak_node::node::NodeCore::new();
        let mut input = oak_node::input::Input::new(
            "tex_in",
            oak_node::value::ValueType::Texture,
            NodeValue::None,
        );
        if !connectable {
            input.flags |= oak_node::input::flags::NOT_CONNECTABLE;
        }
        core.add_input(input);
        core.effect_input = "tex_in".to_string();
        core
    }

    /// The nested-payload recursion ceiling in `bind`: a job box at the
    /// depth limit is left unbound instead of recursing, and the pass
    /// still runs against the placeholders.
    #[test]
    fn gpu_shader_job_depth_ceiling_leaves_the_nested_box_unbound() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let nested = Job::ShaderJob(ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.solidgenerator".into(),
            params: NodeValueRow::from([(
                "color_in".to_string(),
                NodeValue::Color([0.0, 1.0, 0.0, 1.0]),
            )]),
            ..ShaderJobPayload::default()
        });
        let mut params = NodeValueRow::new();
        params.insert(
            "blend_in".into(),
            NodeValue::Texture(oak_node::handle::make_owned(nested)),
        );
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.merge".into(),
            shader_id: String::new(),
            effect_input: String::new(),
            params,
            ..ShaderJobPayload::default()
        };
        let out = RenderEvalHooks::new()
            .process_shader_job_depth(&payload, 8)
            .expect("the pass runs with the nested box unbound");
        assert_eq!(out.size(), (1, 1), "no input bound: the 1x1 fallback");
    }

    /// Inputs whose scratch texture cannot be created (a 0x0 frame) or
    /// uploaded (a frame with a short buffer) are skipped; the pass runs
    /// against the placeholders.
    #[test]
    fn gpu_shader_job_skips_uncreatable_and_unuploadable_inputs() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        // A 0x0 CPU texture: `create_texture` rejects the geometry.
        let zero = Texture::dummy();
        // A 2x2 frame with no payload: `upload` rejects the short buffer.
        let mut short = generate_frame(Rational::new(0, 1), (2, 2), PixelFormat::F32).unwrap();
        short.data.clear();
        let mut params = NodeValueRow::new();
        params.insert("base_in".into(), texture_value(zero));
        params.insert("blend_in".into(), texture_value(Texture::wrap_frame(short)));
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.merge".into(),
            shader_id: String::new(),
            effect_input: String::new(),
            params,
            ..ShaderJobPayload::default()
        };
        let out = RenderEvalHooks::new()
            .process_shader_job(&payload)
            .expect("the pass runs with both bad inputs skipped");
        assert_eq!(out.size(), (1, 1));
    }

    /// A bound token the context does not own fails the pass at run time
    /// (and the reserved output texture is released).
    #[test]
    fn gpu_shader_job_run_failure_reports_none() {
        let Some(ctx) = oak_core::backend::shared_gpu_or_skip("an eval GPU test") else {
            return;
        };
        let bogus = Texture::gpu(ctx, 0xDEAD_BEEF, 4, 4, PixelFormat::F32);
        let mut params = NodeValueRow::new();
        params.insert("base_in".into(), texture_value(bogus));
        let payload = ShaderJobPayload {
            type_id: "org.olivevideoeditor.Olive.merge".into(),
            shader_id: String::new(),
            effect_input: String::new(),
            params,
            ..ShaderJobPayload::default()
        };
        assert!(
            RenderEvalHooks::new().process_shader_job(&payload).is_none(),
            "a foreign token fails the bind group and yields no texture"
        );
    }

    /// A registered node whose shader source does not translate to WGSL
    /// reports a compile failure instead of running.
    #[test]
    fn gpu_shader_job_reports_compile_failures() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let _ = oak_node::factory::Factory::global().register_dynamic(
            oak_node::factory::DynamicNodeMeta {
                type_id: "org.oak.test.badshader".into(),
                name: "BadShader".into(),
                categories: Vec::new(),
                sub_category: String::new(),
                description: String::new(),
                create: Arc::new(|| {
                    (
                        oak_node::node::NodeCore::new(),
                        Box::new(BadShaderBehavior),
                    )
                }),
            },
        );
        let payload = ShaderJobPayload {
            type_id: "org.oak.test.badshader".into(),
            shader_id: String::new(),
            ..ShaderJobPayload::default()
        };
        assert!(
            RenderEvalHooks::new().process_shader_job(&payload).is_none(),
            "an untranslatable shader compiles to nothing"
        );
    }

    /// A color transform on a GPU texture whose token cannot be read back
    /// surfaces the readback error (the LUT pass refused it first).
    #[test]
    fn color_transform_job_reports_a_failed_gpu_readback() {
        if oak_core::backend::shared_gpu_or_skip("an eval GPU test").is_none() {
            return;
        }
        let Some(processor) = red_doubling_processor("badtoken") else {
            return;
        };
        let Some(ctx) = oak_core::backend::GpuContext::shared() else {
            return;
        };
        let input = Texture::gpu(ctx, 0x0BAD_7001, 2, 2, PixelFormat::F32);
        let payload = ColorTransformJobPayload {
            color_processor: Arc::new(processor),
            input: texture_value(input),
            time: Rational::new(0, 1),
        };
        assert!(
            RenderEvalHooks::new()
                .process_color_transform_job(&payload)
                .is_err(),
            "the invalid token cannot be converted"
        );
    }

    /// An imported planar texture whose plane tokens the context does not
    /// own fails the YUV pass and reports the error (the caller then
    /// stages through the CPU decoder).
    #[test]
    fn resolve_planar_footage_reports_a_failed_yuv_pass() {
        let Some(ctx) = oak_core::backend::shared_gpu_or_skip("an eval GPU test") else {
            return;
        };
        let planar = Texture::wrap_planar(oak_core::texture::PlanarTexture::new(
            ctx,
            oak_core::texture::PlanarFormat::Nv12,
            (4, 4),
            (0x0BAD_A001, 0x0BAD_A002),
            oak_core::backend::YuvTransform::bt709_limited(),
            (2, 2),
        ));
        assert!(
            resolve_planar_footage(&planar).is_err(),
            "missing plane tokens fail the planar pass"
        );
    }

    /// A single-sided transition pads the missing side with transparent
    /// black and blends (the head/tail fade); a 0x0 side cannot be padded
    /// and falls through to the native side without a blend job.
    #[test]
    fn blend_transition_fills_a_missing_side_and_skips_the_blend_without_a_pair() {
        let mut graph = oak_node::graph::Graph::new();
        let mut traverser = oak_node::traverser::Traverser::new();
        let mut hooks = RenderEvalHooks::new();

        let from = graph.add_node(
            oak_node::node::NodeCore::new(),
            Box::new(FixedTextureBehavior {
                value: Some(texture_value(filled_frame((4, 4), [1.0, 0.0, 0.0, 1.0]))),
            }),
        );
        let out = blend_transition(
            &graph,
            &mut traverser,
            &mut hooks,
            Some(from),
            None,
            Rational::new(0, 1),
            0.25,
            "crossdissolve",
        )
        .expect("single-sided blend");
        let blended = out.expect("the missing side is filled with black");
        assert_eq!(blended.size(), (4, 4));

        // A 0x0 side: black() cannot create the fill, so the pair match
        // falls through to the native side.
        let dummy = graph.add_node(
            oak_node::node::NodeCore::new(),
            Box::new(FixedTextureBehavior {
                value: Some(texture_value(Texture::dummy())),
            }),
        );
        let out = blend_transition(
            &graph,
            &mut traverser,
            &mut hooks,
            Some(dummy),
            None,
            Rational::new(0, 1),
            0.25,
            "crossdissolve",
        )
        .expect("degenerate single-sided blend");
        let native = out.expect("the native side is returned");
        assert_eq!(native.size(), (0, 0));
    }

    /// The sweep's head table can carry no texture, a null handle, or an
    /// unboxable job handle: all three leave the boundary unchanged.
    #[test]
    fn flush_adjustment_layer_handles_textureless_heads() {
        let cases: [(&str, Option<NodeValue>); 3] = [
            ("none", None),
            ("null", Some(NodeValue::Texture(CHandle::null()))),
            (
                "job",
                Some(NodeValue::Texture(oak_node::handle::make_owned(
                    Job::FootageJob(FootageJobPayload::default()),
                ))),
            ),
        ];
        for (tag, value) in cases {
            let mut graph = oak_node::graph::Graph::new();
            let (acore, abehavior) = oak_node::block::adjustment_create();
            let block = graph.add_node(acore, abehavior);
            let head = graph.add_node(
                effect_head_core(true),
                Box::new(FixedTextureBehavior { value }),
            );
            graph
                .connect(
                    head,
                    block,
                    oak_node::block::adjustment_input::TEXTURE_INPUT,
                    -1,
                )
                .expect("head -> block");
            let mut traverser = oak_node::traverser::Traverser::new();
            let mut hooks = RenderEvalHooks::new();
            let sweep = flush_adjustment_layer(
                &mut graph,
                &mut traverser,
                &mut hooks,
                block,
                &[],
                (4, 4),
                Rational::new(0, 1),
                0.5,
            )
            .expect(tag);
            assert!(sweep.is_none(), "{tag}: the boundary is unchanged");
        }
    }

    /// A chain head whose effect input is not connectable cannot be fed:
    /// the sweep reports the refused connection. The graph driver maps the
    /// same error out of `render_graph_frame`.
    #[test]
    fn flush_adjustment_layer_reports_a_refused_connection() {
        let mut graph = oak_node::graph::Graph::new();
        let (acore, abehavior) = oak_node::block::adjustment_create();
        let block = graph.add_node(acore, abehavior);
        let head = graph.add_node(
            effect_head_core(false),
            Box::new(FixedTextureBehavior {
                value: Some(texture_value(filled_frame((2, 2), [0.0, 0.0, 0.0, 1.0]))),
            }),
        );
        graph
            .connect(
                head,
                block,
                oak_node::block::adjustment_input::TEXTURE_INPUT,
                -1,
            )
            .expect("head -> block");
        let mut traverser = oak_node::traverser::Traverser::new();
        let mut hooks = RenderEvalHooks::new();
        let err = flush_adjustment_layer(
            &mut graph,
            &mut traverser,
            &mut hooks,
            block,
            &[],
            (4, 4),
            Rational::new(0, 1),
            0.5,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot feed"),
            "the error names the refused feed: {err}"
        );
    }

    /// `render_graph_frame` propagates an adjustment sweep failure instead
    /// of dropping the frame.
    #[test]
    fn render_graph_frame_reports_a_refused_adjustment_feed() {
        let project = Project::new();
        let seq;
        {
            let mut p = project.lock().unwrap();
            let graph = &mut p.graph;
            let (score, sbehavior) = oak_node::sequence::SequenceBehavior::create();
            seq = graph.add_node(score, sbehavior);
            let (tlcore, tlbehavior) = oak_node::track::TrackListBehavior::create();
            let tl = graph.add_node(tlcore, tlbehavior);
            let (tcore, tbehavior) = oak_node::track::TrackBehavior::create();
            let track = graph.add_node(tcore, tbehavior);

            let (acore, abehavior) = oak_node::block::adjustment_create();
            let block = graph.add_node(acore, abehavior);
            {
                let a = graph
                    .get_mut(block)
                    .unwrap()
                    .behavior
                    .as_any_mut()
                    .unwrap()
                    .downcast_mut::<oak_node::block::AdjustmentBlockBehavior>()
                    .unwrap();
                a.core.range = TimeRange::new(Rational::new(0, 1), Rational::new(2, 1));
                a.core.enabled = true;
            }
            let head = graph.add_node(
                effect_head_core(false),
                Box::new(FixedTextureBehavior {
                    value: Some(texture_value(filled_frame((4, 4), [0.0, 0.0, 0.0, 1.0]))),
                }),
            );
            graph
                .connect(
                    head,
                    block,
                    oak_node::block::adjustment_input::TEXTURE_INPUT,
                    -1,
                )
                .expect("head -> block");
            graph
                .get_mut(track)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackBehavior>()
                .unwrap()
                .append_block(block);
            graph
                .get_mut(tl)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::track::TrackListBehavior>()
                .unwrap()
                .tracks
                .push(track);
            graph
                .get_mut(seq)
                .unwrap()
                .behavior
                .as_any_mut()
                .unwrap()
                .downcast_mut::<oak_node::sequence::SequenceBehavior>()
                .unwrap()
                .track_lists
                .push(tl);
        }
        let err = render_graph_frame(
            &project,
            seq,
            Rational::new(0, 1),
            (4, 4),
            PixelFormat::F32,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("cannot feed"),
            "the sweep error reaches the caller: {err}"
        );
    }

    /// A montage clip whose media cannot be opened propagates the decode
    /// error rather than compositing a hole.
    #[test]
    fn montage_reports_a_failed_clip_decode() {
        let path = std::env::temp_dir().join(format!(
            "oakrender_missing_montage_{}.mp4",
            std::process::id()
        ));
        let params = montage_params_with_effect(&path.to_string_lossy(), "com.example.unused");
        let mut dst = vec![0u8; 16 * 16 * 16];
        assert!(
            render_montage_frame_into(
                Rational::new(0, 1),
                &params,
                (16, 16),
                &mut dst,
                256
            )
            .is_err(),
            "a missing clip file fails the montage frame"
        );
    }

    /// A clip effect that hands back a GPU texture on a context whose
    /// readback fails surfaces the readback error.
    #[test]
    fn montage_reports_a_failed_gpu_readback() {
        let _env = ENV_TEST_LOCK.lock().unwrap();
        let _guard = PLUGIN_TEST_LOCK.lock().unwrap();
        let path = std::env::temp_dir().join(format!(
            "oakrender_montage_readback_{}.mp4",
            std::process::id()
        ));
        oak_codec::testmedia::write_test_clip(&path, 16, 16, 10, 10).expect("test clip");
        crate::ofxhost::install_client(None);
        set_plugin_instance_factory(Some(Arc::new(|_id: &str| Some(7))));
        set_plugin_executor(Some(Arc::new(|req: &PluginJobRequest<'_>| {
            Ok(Texture::gpu(
                Arc::new(UnusedCtx),
                0xE1,
                req.src.size().0,
                req.src.size().1,
                PixelFormat::F32,
            ))
        })));
        let params = montage_params_with_effect(&path.to_string_lossy(), "com.example.no-readback");
        let mut dst = vec![0u8; 16 * 16 * 16];
        let result = render_montage_frame_into(
            Rational::new(0, 1),
            &params,
            (16, 16),
            &mut dst,
            256,
        );
        set_plugin_executor(None);
        set_plugin_instance_factory(None);
        let _ = std::fs::remove_file(&path);
        assert!(
            result.is_err(),
            "an unreadable GPU effect result fails the montage frame"
        );
    }

    /// Degenerate adjustment-span geometry is inert: a zero width cannot
    /// stage a frame, and a destination too small for the staged copy is
    /// left untouched.
    #[test]
    fn adjustment_span_degenerate_geometry_is_inert() {
        let span = adjustment_span(
            0,
            2,
            0,
            vec![unknown_effect("com.example.span-geometry")],
        );

        // w == 0: the staging frame cannot be generated.
        let mut dst = vec![0xABu8; 16];
        apply_adjustment_span(&mut dst, 16, 0, 1, &span, Rational::new(0, 1));
        assert!(dst.iter().all(|&b| b == 0xAB), "zero width is inert");

        // The destination is shorter than the staged frame geometry.
        let mut dst = vec![0xCDu8; 8];
        apply_adjustment_span(&mut dst, 8, 2, 1, &span, Rational::new(0, 1));
        assert!(dst.iter().all(|&b| b == 0xCD), "a short buffer is inert");
    }

    /// The tests' GPU stand-ins and the null-texture behavior expose their
    /// documented error/duplication contracts.
    #[test]
    fn test_context_stubs_report_their_contracts() {
        use oak_core::backend::GpuContextLike;
        use oak_node::node::NodeBehavior;

        let unused = UnusedCtx;
        assert!(unused.upload(0, &Frame::dummy()).is_err());
        assert!(unused.download(0).is_err());
        assert!(unused.blit(0, 0, None).is_err());
        unused.destroy_texture(0);

        let echo = FrameEchoCtx {
            frame: Frame::dummy(),
        };
        assert!(echo.upload(7, &Frame::dummy()).is_ok());
        assert_eq!(echo.download(7).unwrap().width, 0);
        assert!(echo.blit(0, 0, None).is_err());

        let behavior = NullTextureBehavior;
        assert_eq!(behavior.name(), "NullTexture");
        assert_eq!(behavior.type_id(), "org.oak.test.nulltexture");
        assert!(behavior.duplicate(&oak_node::node::NodeCore::new()).is_some());
    }

}

