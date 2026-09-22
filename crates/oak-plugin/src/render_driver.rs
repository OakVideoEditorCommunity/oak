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

//! render 驱动：`src/render/src/plugin/pluginrenderer.cpp`
//! `PluginRenderer::render_plugin`（1857 行 C++ 的一部分）的渲染流程
//! 语义收编（M11 §4）。
//!
//! 目标：oakrender 的 PluginJob 语义全部收编进本模块（单库化后
//! 原 `oakplugin_instance_render_job` C ABI 已删除，facade 直接构造
//! [`RenderJob`] 调 [`render_frame`]），本模块承载全部 OFX 宿主
//! 渲染流程。逐段对照的 C++ 行号已注释。
//!
//! ## 流程（render_frame，对应 render_plugin）
//!
//! 1. 实例锁（pluginrenderer.cpp:1436-1444）+ 取消检查；
//! 2. use_opengl 决策（pluginrenderer.cpp:1446-1457）：插件描述符
//!    kOfxImageEffectPropOpenGLRenderSupported ∈ {true, needed} 且
//!    渲染器是 OpenGL 且目标纹理有 GL id；
//! 3. 输入纹理收集（pluginrenderer.cpp:1518-1552）：effect_input_id
//!    匹配 → job.src；否则按 clip 名的纹理表；SimpleSource 回退；
//! 4. getClipPreferences（pluginrenderer.cpp:1554-1594）；
//! 5. RoI/RoD 设定（pluginrenderer.cpp:1596-1607）：region_of_interest
//!    = 目标尺寸（规范坐标）；输出 clip 的 RoD 与输出纹理挂接；
//! 6. 输入 clip 的 RoD 与格式（pluginrenderer.cpp:1627-1665；Phase 2
//!    全链路 F32 → 无转换）；
//! 7. getRegionOfInterest（pluginrenderer.cpp:1666-1680）：RoI 为
//!    建议值，任何失败按默认整帧继续（C++ 对 BadHandle 即如此；
//!    Phase 2 其余状态同样继续——RoI 仅影响上游渲染范围，本管线
//!    输入由 oakrender 整帧提供）；
//! 8. 输出 clip 格式（pluginrenderer.cpp:1686-1697）；
//! 9. **isIdentity 短路**（ofxRendering "Identity Effects"）：
//!    isIdentity 命中 → 直接把所引输入 clip 在透传时间的帧拷入
//!    输出（不调 render action）；
//! 10. 参数覆盖（pluginrenderer.cpp:132-290 apply_param_overrides）；
//! 11. render action：CPU 路径经 [`crate::instance::Instance::render`]
//!     （输出装配：图像 → 目标纹理帧，行跨度感知）；GL 路径经
//!     [`crate::instance::Instance::render_gl`] + [`crate::gl_bridge`]
//!     （方案 B：宿主建离屏上下文，插件画进 FBO 附着的输出 GL 纹理，
//!     render 返回后 glReadPixels 回读装配——输出格式与 CPU 路径一致；
//!     GL 失败回退 CPU）。
//!
//! ## begin/end 序列括号
//!
//! ofxRendering 文档："All calls to the render action are bracketed by
//! a pair of begin/end sequence render actions"。oakrender 对同一实例
//! 的一批帧先 [`begin_sequence`] 后 [`end_sequence`]，中间逐帧
//! [`render_frame`]。

use crate::image::Image;
use crate::instance::{Instance, OfxRangeD, OfxRectD, RenderScale};
use crate::property::Value;
use crate::render::{self, Renderer, Texture};

/// 一帧渲染任务的输入（oakrender PluginJob 的 Rust 侧视图）。
pub struct RenderJob {
	/// 帧时间（秒）。
	pub time: f64,
	/// 目标纹理（输出；oakrender 侧创建并经值传入）。
	pub dst: Texture,
	/// 主输入纹理（effect_input_id / SimpleSource；无则 None）。
	pub src: Option<Texture>,
	/// effect 输入 clip 名（job.src 的落点；C++ `node->get_effect_input_id()`）。
	pub effect_input_id: Option<String>,
	/// 其余输入 clip 的纹理表（clip 名 → 纹理）。
	pub inputs: Vec<(String, Texture)>,
	/// 参数覆盖（参数名 → oaknode_value POD；对应 NodeValueRow）。
	pub values: Vec<(String, crate::node::Value)>,
	/// GL 渲染器（None → CPU 路径；Some → 视 use_opengl 决策）。
	pub renderer: Option<Renderer>,
	/// 渲染前是否清空目标（信息性——C++ render_plugin 亦不处理，
	/// 由上层渲染器负责；见插件渲染器注释）。
	pub clear_destination: bool,
	/// 交互式渲染标记（信息性；Phase 2 的 render in args 恒 0，见
	/// [`crate::instance::Instance::render`]）。
	pub interactive: bool,
}

impl Default for RenderJob {
	fn default() -> Self {
		Self {
			time: 0.0,
			dst: Texture::dummy(),
			src: None,
			effect_input_id: None,
			inputs: Vec::new(),
			values: Vec::new(),
			renderer: None,
			clear_destination: false,
			interactive: false,
		}
	}
}

/// beginSequenceRender 括号（ofxRendering："bracketed by a pair of
/// begin/end sequence render actions"；ofxGPURender.h 的 GL 模式下该
/// action 的 in args 也带 kOfxImageEffectPropOpenGLEnabled）。
/// `gl` 非空时按 GL 模式调用。
pub fn begin_sequence(
	inst: &Instance,
	range: OfxRangeD,
	gl: Option<Renderer>,
) -> crate::error::Result<()> {
	if gl.is_some() {
		// GL 模式：ofxGPURender.h "OpenGL Current Context" 要求
		// BeginSequenceRender 期间上下文 current（插件可能在此分配 GL
		// 资源）。acquire 覆盖整个 action；返回即清 current。
		let _guard = crate::gl_bridge::acquire()?;
		inst.begin_sequence_render_gl(range)
	} else {
		inst.begin_sequence_render(range)
	}
}

/// endSequenceRender 括号（与 [`begin_sequence`] 配对）。
pub fn end_sequence(
	inst: &Instance,
	range: OfxRangeD,
	gl: Option<Renderer>,
) -> crate::error::Result<()> {
	if gl.is_some() {
		let _guard = crate::gl_bridge::acquire()?;
		inst.end_sequence_render_gl(range)
	} else {
		inst.end_sequence_render(range)
	}
}

/// 渲染一帧（`render_plugin` 的 Rust 移植；逐段行号对照见模块文档）。
///
/// 返回装配完成的输出纹理与各输入 clip 的 RoI（clip 名 → 矩形；
/// 值型纹理下输出写入 `job.dst` 的本地副本并随返回值交付——CPU
/// 纹理的 `to_frame` 是拷贝，就地写不回只读的 `job.dst`）。宿主不
/// 据 RoI 裁剪输入——输入由 oakrender 整帧提供，与 C++ 渲染器行为
/// 一致。
pub fn render_frame(
	inst: &Instance,
	job: &RenderJob,
) -> crate::error::Result<(Texture, Vec<(String, OfxRectD)>)> {
	use crate::error::Error;

	// 1. 实例锁（pluginrenderer.cpp:1436-1444：OlivePluginInstance 非
	// 线程安全，并发 setInputTexture/renderAction 会毁内部状态）。
	let _lock = inst.render_lock.lock().unwrap_or_else(|e| e.into_inner());
	// 取消检查必须在任何插件调用之前（shutdown 后入口已卸载）。
	if inst.cancel.load(std::sync::atomic::Ordering::Relaxed) {
		return Err(Error::Failed("已取消".into()));
	}

	// 2. use_opengl（pluginrenderer.cpp:1446-1457）：插件声明 GL 支持
	// 且渲染器是 OpenGL 且像素深度协商可行（管线 F32 满足插件
	// kOfxOpenGLPropPixelDepth 声明）且目标纹理有有效 GL 名（= 桥能为
	// 目标帧建出真实 GL 纹理 ⟺ 离屏上下文可用，[`crate::gl_bridge`]）。
	// 任一不满足 → 回退 CPU 路径（GL 分支在下方真正渲染）。
	let use_opengl = match job.renderer.as_ref() {
		Some(r) if render::renderer_is_open_gl(r) => {
			let plugin_gl = plugin_supports_opengl(inst);
			let depth_ok =
				crate::suites::gl_render::pick_gl_pixel_depth(&inst.plugin.descriptor.props)
					.is_some();
			let gl_name_ok = crate::gl_bridge::gl_available();
			if std::env::var_os("OAK_OFX_TRACE").is_some() {
				let raw = inst
					.plugin
					.descriptor
					.props
					.get(crate::host::PROP_GL_RENDER_SUPPORTED, 0);
				eprintln!(
					"[ofx] use_opengl decision: plugin_gl={plugin_gl} depth_ok={depth_ok} gl_available={gl_name_ok} (raw GL prop: {raw:?})"
				);
			}
			plugin_gl && depth_ok && gl_name_ok
		}
		_ => false,
	};

	// 目标帧与参数（F32 校验；输出装配的依据）。dst 取本地副本：
	// 值型纹理下输出装配写回副本，随返回值交付（job.dst 只读）。
	let (dst_params, w, h) = read_dst(&job.dst)?;
	let mut dst = job.dst.clone();
	let par = pixel_aspect(&dst_params);
	// 规范坐标的 RoI/RoD（pluginrenderer.cpp:1595-1603：x2 = 宽 × PAR）。
	let region_of_interest = OfxRectD {
		x1: 0.0,
		y1: 0.0,
		x2: w * par,
		y2: h,
	};

	// 3. 输入纹理收集（pluginrenderer.cpp:1518-1552）。
	for clip in &inst.clips {
		if clip.name == "Output" {
			continue;
		}
		if let Some(tex) = pick_input(&clip.name, job) {
			if usable(&tex) {
				clip.set_input_texture(Some(tex), job.time);
			}
		}
	}

	// 4. getClipPreferences（pluginrenderer.cpp:1554-1594）。
	let prefs = inst.get_clip_preferences()?;
	if std::env::var_os("OAK_OFX_TRACE").is_some() {
		eprintln!("[ofx] clip prefs: components={} (inst {})", prefs.output_components, inst.plugin.identifier);
	}
	let components = match prefs.output_components.as_str() {
		"OfxImageComponentRGBA" => crate::image::Components::Rgba,
		"OfxImageComponentRGB" => crate::image::Components::Rgb,
		"OfxImageComponentAlpha" => crate::image::Components::Alpha,
		_ => return Err(Error::Failed("协商分量未知".into())),
	};
	// 位深协商（设计约束：管线全链路 ACEScg + F32；插件不支持 F32
	// 时按插件协商的位深渲染——输入图像转低、输出转回 F32，而不是
	// 直接失败）。GL 路径强制 F32：GL 纹理按管线格式创建，位深由
	// kOfxOpenGLPropPixelDepth 另行协商，clip props 必须与插件实际
	// 见到的图像一致。olive 像素格式号：0=u8, 2=u16, 3=f16, 4=f32。
	let (depth, depth_format) = if use_opengl {
		(crate::image::BitDepth::Float, 4)
	} else {
		let d = crate::image::BitDepth::from_ofx(&prefs.output_bit_depth)
			.ok_or_else(|| Error::Failed(format!("协商位深未知：{}", prefs.output_bit_depth)))?;
		let f = match d {
			crate::image::BitDepth::Byte => 0,
			crate::image::BitDepth::Short => 2,
			crate::image::BitDepth::Half => 3,
			crate::image::BitDepth::Float => 4,
		};
		(d, f)
	};

	// 5. 输出 clip：RoD + 输出纹理挂接（pluginrenderer.cpp:1603-1606）。
	let output_clip = inst
		.clips
		.iter()
		.find(|c| c.name == "Output")
		.ok_or_else(|| Error::Failed("实例无 Output clip".into()))?;
	output_clip.set_region_of_definition(region_of_interest, job.time);
	output_clip.set_output_texture(Some(dst.clone()), job.time);

	// 6. 输入 clip：RoD 与格式（pluginrenderer.cpp:1627-1665；位深按
	// 协商结果——非 F32 插件的输入图像在 fetch 时转低）。
	for clip in &inst.clips {
		if clip.name == "Output" {
			continue;
		}
		if pick_input(&clip.name, job).is_some_and(|t| usable(&t)) {
			clip.set_region_of_definition(region_of_interest, job.time);
			clip.set_video_params(depth_format, 4);
		}
	}

	// 7. getRegionOfInterest（pluginrenderer.cpp:1666-1680）。RoI 为
	// 建议值：失败按默认整帧继续（C++ 对 BadHandle 如此；本管线
	// 输入整帧提供，RoI 只作记录）。
	let rois = inst
		.get_regions_of_interest(job.time, RenderScale { x: 1.0, y: 1.0 }, region_of_interest)
		.unwrap_or_else(|_| inst.clips.iter().map(|_| region_of_interest).collect());

	// 8. 输出 clip 格式（pluginrenderer.cpp:1686-1697；位深按协商）。
	output_clip.set_video_params(depth_format, components.channel_count() as i32);

	// 渲染窗口（像素坐标；pluginrenderer.cpp:1699-1704）。
	let render_window = OfxRectD {
		x1: 0.0,
		y1: 0.0,
		x2: w,
		y2: h,
	};

	// 9. isIdentity 短路（ofxRendering "Identity Effects"）：插件声明
	// 本帧等价于某输入 clip → 直接透传该 clip 在透传时间的帧。
	if let Some((t, clip_name)) = inst.is_identity(job.time)? {
		passthrough(inst, &clip_name, t, &mut dst)?;
		return Ok((dst, zip_rois(inst, &rois)));
	}

	// 10. 参数覆盖（pluginrenderer.cpp:1729-1731 + 132-290）。
	apply_param_overrides(inst, &job.values);

	// 11. render action。
	if !use_opengl {
		let output = std::sync::Arc::new(Image::allocate(
			depth,
			components,
			OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: w,
				y2: h,
			},
		));
		inst.render(
			job.time,
			RenderScale { x: 1.0, y: 1.0 },
			render_window,
			output.clone(),
		)?;
		if std::env::var_os("OAK_OFX_TRACE").is_some() && depth == crate::image::BitDepth::Float {
			let p = output.pixels();
			let dump: Vec<f32> = p
				.chunks_exact(4)
				.take(4)
				.map(|c| f32::from_le_bytes(c.try_into().unwrap()))
				.collect();
			eprintln!("[ofx] render_frame: output buffer at {:p}, pixels[0..4] = {dump:?}", p.as_ptr());
		}
		// 输出装配（pluginrenderer.cpp:1762-1834 的 CPU 路径）：插件
		// 协商了低位深时先转回管线工作格式（全链路 F32/ACEScg）。
		let f32_output;
		let output_f32 = if depth == crate::image::BitDepth::Float {
			&*output
		} else {
			f32_output = output.convert_depth(crate::image::BitDepth::Float);
			&f32_output
		};
		write_output_frame(&mut dst, output_f32)?;
	} else {
		// GL 路径（方案 B，见 [`crate::gl_bridge`]）：宿主自建离屏
		// 上下文，为输出帧建 GL 纹理 + FBO（插件直接画进附着的输出
		// 纹理，等价 C++ attach_output_texture），render 返回后
		// glReadPixels 回读装帧（pluginrenderer.cpp:1784-1834 的 GL
		// 分支 + 本实现的回读装配）。GL 失败回退 CPU（对齐现有失败
		// 语义；最终失败仍上抛）。
		match render_gl_frame(
			inst,
			job,
			RenderScale { x: 1.0, y: 1.0 },
			render_window,
			w,
			h,
			&dst_params,
		) {
			Ok(image) => write_output_frame(&mut dst, &image)?,
			Err(gl_err) => {
				eprintln!("[PLUGIN] GL 渲染失败，回退 CPU：{gl_err}");
				let output = std::sync::Arc::new(Image::allocate(
					depth,
					components,
					OfxRectD {
						x1: 0.0,
						y1: 0.0,
						x2: w,
						y2: h,
					},
				));
				inst.render(
					job.time,
					RenderScale { x: 1.0, y: 1.0 },
					render_window,
					output.clone(),
				)?;
				// 同主 CPU 路径：低位深协商的输出先转回 F32 再装帧。
				let f32_output;
				let output_f32 = if depth == crate::image::BitDepth::Float {
					&*output
				} else {
					f32_output = output.convert_depth(crate::image::BitDepth::Float);
					&f32_output
				};
				write_output_frame(&mut dst, output_f32)?;
			}
		}
	}

	Ok((dst, zip_rois(inst, &rois)))
}

/// GL 渲染一帧（render_frame 的 GL 分支主体；返回回读装配完成的
/// F32 RGBA 图像）。
///
/// 流程：acquire 离屏上下文（本线程 current，全局串行）→ 建输出 GL
/// 纹理（尺寸 = 目标帧，格式按 `dst_params`）→ FBO 挂载并绑定 →
/// 视口 → [`Instance::render_gl`]（插件画进附着纹理；真实纹理名经
/// GlCtx 注入，clipLoadTexture(Output) 返回它）→ glReadPixels 回读
/// 装配（垂直翻转 + 格式转换）→ 清理 FBO/纹理。任何一步失败返回
/// Err，调用方回退 CPU。
fn render_gl_frame(
	inst: &Instance,
	job: &RenderJob,
	scale: RenderScale,
	window: OfxRectD,
	w: f64,
	h: f64,
	dst_params: &render::VideoParams,
) -> crate::error::Result<std::sync::Arc<Image>> {
	use crate::error::Error;

	// 离屏上下文（进程级共享；本线程 current 直到 guard drop）。
	let _guard = crate::gl_bridge::acquire()?;
	let (wpx, hpx) = (w as i32, h as i32);
	// 输出 GL 纹理 + FBO 挂载（GL_RGBA32F/GL_RGBA8 按目标帧格式）。
	let gl_tex = crate::gl_bridge::create_output_texture(wpx, hpx, dst_params)?;
	let fbo = crate::gl_bridge::create_fbo(gl_tex, wpx, hpx)?;
	crate::gl_bridge::bind_fbo(fbo);
	crate::gl_bridge::set_viewport(wpx, hpx);

	// render action（OpenGLEnabled=1；输出纹理真实名经 GlCtx 注入）。
	let render_res = inst.render_gl(
		job.time,
		scale,
		window,
		job.renderer.clone().unwrap(),
		job.dst.clone(),
		Some(gl_tex),
	);

	// 回读前防御性重绑 FBO + 视口（规范下插件不解除输出绑定，但个别
	// 插件可能改绑/改视口——重绑保证 glReadPixels 读的是输出纹理）。
	crate::gl_bridge::bind_fbo(fbo);
	crate::gl_bridge::set_viewport(wpx, hpx);

	// 回读（FBO 仍绑定、输出 GL 纹理仍存活）。
	let readback = crate::gl_bridge::read_pixels_to_image(wpx, hpx, dst_params);

	// 清理（guard drop 时清 current 并放锁）。
	crate::gl_bridge::delete_fbo(fbo);
	crate::gl_bridge::delete_gl_texture(gl_tex);

	render_res?;
	readback.map_err(|e| Error::Failed(format!("GL 回读失败：{e}")))
}

/// 把输入 clip 名与 RoI 列表配对（与 `clips` 顺序一致）。
fn zip_rois(inst: &Instance, rois: &[OfxRectD]) -> Vec<(String, OfxRectD)> {
	inst.clips
		.iter()
		.enumerate()
		.filter(|(_, c)| c.name != "Output")
		.map(|(i, c)| (c.name.clone(), rois.get(i).copied().unwrap_or_default()))
		.collect()
}

/// 读目标纹理的参数（F32 校验）。返回 (参数, 宽, 高)。
fn read_dst(dst: &Texture) -> crate::error::Result<(render::VideoParams, f64, f64)> {
	use crate::error::Error;
	let params = render::texture_get_params(dst);
	if params.format != render::PIXEL_FORMAT_F32 {
		return Err(Error::Failed(format!(
			"输出帧格式 {} 非 F32（Phase 2 约束）",
			params.format
		)));
	}
	let (w, h) = (params.width as f64, params.height as f64);
	if w <= 0.0 || h <= 0.0 {
		return Err(Error::Invalid);
	}
	Ok((params, w, h))
}

/// 目标参数的像素比（缺失 1.0）。
fn pixel_aspect(params: &render::VideoParams) -> f64 {
	if params.pixel_aspect_den != 0 {
		params.pixel_aspect_num as f64 / params.pixel_aspect_den as f64
	} else {
		1.0
	}
}

/// 插件描述符是否声明 GL 渲染支持（ofxGPURender.h:397-408：
/// "true"/"needed"）。
fn plugin_supports_opengl(inst: &Instance) -> bool {
	inst.plugin
		.descriptor
		.props
		.get(crate::host::PROP_GL_RENDER_SUPPORTED, 0)
		.map(|v| match v {
			Value::String(s) => {
				let s = s.to_string_lossy();
				s == "true" || s == "needed"
			}
			_ => false,
		})
		.unwrap_or(false)
}

/// 输入纹理是否可用（非占位；pluginrenderer.cpp:1504-1513 的
/// is_usable_input——Phase 2 只看非 dummy，帧/Renderer 由 oakrender
/// 保证）。
fn usable(tex: &Texture) -> bool {
	!tex.is_dummy()
}

/// 按 C++ pluginrenderer.cpp:1527-1543 的规则选输入纹理。
fn pick_input(clip_name: &str, job: &RenderJob) -> Option<Texture> {
	if job.effect_input_id.as_deref() == Some(clip_name) {
		if let Some(src) = &job.src {
			return Some(src.clone());
		}
	}
	for (name, tex) in &job.inputs {
		if name == clip_name {
			return Some(tex.clone());
		}
	}
	// SimpleSource 回退（pluginrenderer.cpp:1534-1543：
	// kOfxImageEffectSimpleSourceClipName 取 k_texture_input，再回退
	// job.src）。
	if clip_name == "Source" {
		return job.src.clone();
	}
	None
}

/// isIdentity 透传：把所引输入 clip 在 `t` 的帧拷入输出（CPU 拷贝；
/// 目标帧 F32 校验）。
fn passthrough(
	inst: &Instance,
	clip_name: &str,
	t: f64,
	dst: &mut Texture,
) -> crate::error::Result<()> {
	use crate::error::Error;
	let clip = inst
		.clips
		.iter()
		.find(|c| c.name == clip_name)
		.ok_or_else(|| Error::Failed(format!("isIdentity 引用未知 clip {clip_name}")))?;
	let image = clip.fetch_image(t, RenderScale { x: 1.0, y: 1.0 }, None)?;
	write_output_frame(dst, &image)
}

/// 参数覆盖（pluginrenderer.cpp:132-290 `apply_param_overrides` 的
/// Rust 移植）：把每帧的节点值注入实例参数。字符串族（String/
/// StrChoice）经专用路径（set_param_string），不在此表的 oaknode
/// POD 表达范围内 → 跳过（与 C++ 的 k_file/k_text/k_font/k_str_combo
/// 走专用桥一致）。
fn apply_param_overrides(inst: &Instance, values: &[(String, crate::node::Value)]) {
	for (key, v) in values {
		let Some(p) = inst.params.find(key) else {
			continue;
		};
		let Some(mut pv) = crate::param::param_value_from_node(v, &p.def.ofx_type) else {
			continue;
		};
		// Double 标量的 NaN/Inf 清洗 + Min/Max 钳制
		// （pluginrenderer.cpp:155-177：坏值回退默认并告警，再按
		// kOfxParamPropMin/Max 钳制；多维 Double 族 C++ 无此检查）。
		if p.def.ofx_type == crate::param::TYPE_DOUBLE {
			if let crate::param::ParamValue::Double(d, 1) = &mut pv {
				if d[0].is_nan() || d[0].is_infinite() {
					eprintln!(
						"[PLUGIN] NaN/Inf in double param {key} replacing with default"
					);
					d[0] = prop_double(&p.def.props, crate::param::P_DEFAULT, 0);
				}
				if let Some(Value::Double(min)) = p.def.props.get(crate::param::P_MIN, 0) {
					if d[0] < min {
						d[0] = min;
					}
				}
				if let Some(Value::Double(max)) = p.def.props.get(crate::param::P_MAX, 0) {
					if d[0] > max {
						d[0] = max;
					}
				}
			}
		}
		p.set_ofx(pv);
	}
}

/// 读属性的 Double 值（缺失 0.0；Int 提升）。
fn prop_double(props: &crate::property::PropertySet, name: &str, index: usize) -> f64 {
	match props.get(name, index) {
		Some(Value::Double(v)) => v,
		Some(Value::Int(v)) => v as f64,
		_ => 0.0,
	}
}

/// 把 CPU 图像写入目标纹理（行优先、行跨度感知；F32 校验）。
/// Phase 2 输出装配的公共落点（CPU render 路径与 isIdentity 透传
/// 共用）。GPU 目标纹理经后端 upload 回写（`Texture::Gpu` 分支）。
pub(crate) fn write_output_frame(dst: &mut Texture, image: &Image) -> crate::error::Result<()> {
	use crate::error::Error;
	// CPU 纹理就地写入；GPU 纹理经下载帧改写后 upload 回写。
	let mut frame = render::texture_get_frame(dst)?;
	let params = frame.video_params();
	if params.format != render::PIXEL_FORMAT_F32 {
		return Err(Error::Failed("输出帧格式非 F32（Phase 2 约束）".into()));
	}
	let (w, h) = (params.width as usize, params.height as usize);
	let tight = w * image.components().channel_count() * 4;
	if tight != image.row_bytes() || tight * h != image.pixels().len() {
		return Err(Error::Failed("图像尺寸与输出帧不一致".into()));
	}
	let dst_ptr = frame.data_mut();
	if dst_ptr.is_null() {
		return Err(Error::Failed("输出帧无数据".into()));
	}
	let row = frame.linesize_bytes();
	let row = if row > 0 { row } else { tight };
	let dst_bytes = unsafe { std::slice::from_raw_parts_mut(dst_ptr, row * h) };
	let pixels = image.pixels();
	for y in 0..h {
		let d = y * row;
		let s = y * tight;
		dst_bytes[d..d + tight].copy_from_slice(&pixels[s..s + tight]);
	}
	match dst {
		// 就地写回（值型 CPU 纹理：to_frame 是拷贝，必须写回本体）。
		Texture::Cpu(f) => {
			f.data = frame.data;
		}
		// GPU 目标纹理：拷贝只落在下载帧上，经后端 upload 回写。
		Texture::Gpu { token, ctx, .. } => {
			ctx.upload(*token, &frame)
				.map_err(|e| Error::Failed(format!("输出纹理上传失败：{e}")))?;
		}
		// 未解析的平面纹理不会成为插件输出目标（解码路径会先解析）。
		Texture::Planar(_) => {
			return Err(Error::Failed("平面纹理不能作为插件输出目标".into()));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::descriptor::{ClipDescriptor, EffectDescriptor};
	use crate::error::Error;
	use crate::instance::Instance;
	use crate::param::{ParamDef, ParamInstance, ParamSetInstance, ParamValue};
	use crate::property::{PropertySet, Value};

	fn cs(s: &str) -> std::ffi::CString {
		std::ffi::CString::new(s).unwrap()
	}

	fn f32_frame(w: i32, h: i32, fill: f32) -> crate::render::Frame {
		let params = render::VideoParams {
			width: w,
			height: h,
			format: render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let mut frame = crate::render::Frame::new();
		frame.set_video_params(params);
		assert!(frame.allocate());
		for c in frame.data.as_chunks_mut::<4>().0 {
			c.copy_from_slice(&fill.to_le_bytes());
		}
		frame
	}

	fn f32_texture(w: i32, h: i32, fill: f32) -> Texture {
		Texture::wrap_frame(f32_frame(w, h, fill))
	}

	fn u8_texture(w: i32, h: i32) -> Texture {
		let params = render::VideoParams {
			width: w,
			height: h,
			format: render::PIXEL_FORMAT_U8,
			..Default::default()
		};
		let pixels = vec![0u8; w as usize * h as usize * 4];
		render::texture_create(&params, &pixels, 0).expect("U8 纹理应可建")
	}

	fn f32_image(w: i32, h: i32, fill: f32) -> Image {
		let mut img = Image::allocate(
			crate::image::BitDepth::Float,
			crate::image::Components::Rgba,
			OfxRectD {
				x1: 0.0,
				y1: 0.0,
				x2: w as f64,
				y2: h as f64,
			},
		);
		for c in img.pixels_mut().as_chunks_mut::<4>().0 {
			c.copy_from_slice(&fill.to_le_bytes());
		}
		img
	}

	/// 测试入口：按实例 props 里的 test.* 配置扮演插件行为。实例句柄
	/// （offset 0）即配置存储，避免测试间共享全局状态。
	unsafe extern "C" fn test_entry(
		action: *const std::ffi::c_char,
		handle: *const std::ffi::c_void,
		in_args: *mut std::ffi::c_void,
		out_args: *mut std::ffi::c_void,
	) -> std::ffi::c_int {
		let action_name = unsafe { std::ffi::CStr::from_ptr(action) }
			.to_string_lossy()
			.into_owned();
		let inst_props = unsafe {
			&*(crate::suites::tag::strip(handle as *mut std::ffi::c_void))
		};
		let in_args = unsafe { &*(in_args as *const PropertySet) };
		let out_args = unsafe { &*(out_args as *const PropertySet) };

		if action_name == crate::host::ACTION_GET_CLIP_PREFERENCES {
			if let Some(Value::String(comps)) = inst_props.get("test.components", 0) {
				out_args.set_one(
					&crate::host::clip_pref_prop(crate::host::CLIP_PREF_COMPONENTS, "Output"),
					Value::String(comps.clone()),
				);
			}
			if let Some(Value::String(depth)) = inst_props.get("test.depth", 0) {
				out_args.set_one(
					&crate::host::clip_pref_prop(crate::host::CLIP_PREF_DEPTH, "Output"),
					Value::String(depth.clone()),
				);
			}
		} else if action_name == crate::host::ACTION_IS_IDENTITY {
			if let Some(Value::String(clip)) = inst_props.get("test.identity_clip", 0) {
				if !clip.to_string_lossy().is_empty() {
					out_args.set_one(crate::host::PROP_IS_IDENTITY, Value::String(clip.clone()));
					if let Some(Value::Double(t)) = inst_props.get("test.identity_time", 0) {
						out_args.set_one(crate::host::PROP_TIME, Value::Double(t));
					}
				}
			}
		} else if action_name == crate::host::ACTION_RENDER {
			// GL 失败注入：仅 GL 模式的 render（in args 带 OpenGLEnabled=1）。
			let gl_enabled = matches!(
				in_args.get(crate::host::PROP_GL_ENABLED, 0),
				Some(Value::Int(1))
			);
			if gl_enabled && matches!(inst_props.get("test.gl_fail", 0), Some(Value::Int(1))) {
				return crate::suites::status::FAILED;
			}
			if let Some(Value::Int(st)) = inst_props.get("test.render_status", 0) {
				return st;
			}
		}
		crate::suites::status::OK
	}

	#[allow(clippy::vec_box)] // ParamSetInstance 的字段类型即 Vec<Box<ParamInstance>>。
	fn make_instance(
		desc_props: Vec<(&'static str, Value)>,
		inst_props: Vec<(&'static str, Value)>,
		clips: &[&str],
		params: Vec<Box<ParamInstance>>,
	) -> Instance {
		let desc = EffectDescriptor::new();
		for (k, v) in desc_props {
			desc.props.set_one(k, v);
		}
		let props = PropertySet::new();
		for (k, v) in inst_props {
			props.set_one(k, v);
		}
		Instance {
			props,
			plugin: std::sync::Arc::new(crate::host::Plugin {
				identifier: "org.oak.test-render-driver".into(),
				version: (1, 0),
				bundle_path: std::path::PathBuf::new(),
				contexts: vec![],
				descriptor: desc,
				lib: std::ptr::null_mut(),
				entry: test_entry,
				ofx_plugin: std::ptr::null_mut(),
				unloaded: std::sync::atomic::AtomicBool::new(false),
			}),
			context: "OfxImageEffectContextFilter".into(),
			params: ParamSetInstance { params },
			clips: clips
				.iter()
				.map(|n| Box::new(crate::clip::ClipInstance::from_descriptor(&ClipDescriptor::new(n))))
				.collect(),
			node_identity: std::sync::atomic::AtomicUsize::new(0),
			// 置位销毁门：测试实例 drop 时不再向假插件发 destroyInstance。
			destroyed: std::sync::atomic::AtomicBool::new(true),
			sequence_range: std::sync::Mutex::new(None),
			progress_cb: std::sync::Mutex::new(None),
			cancel: std::sync::atomic::AtomicBool::new(false),
			edit: std::sync::Mutex::new(crate::instance::EditTransaction::new()),
			render_lock: std::sync::Mutex::new(()),
			interact: std::sync::Mutex::new(None),
		}
	}

	fn job(dst: Texture, src: Option<Texture>) -> RenderJob {
		RenderJob {
			time: 0.0,
			dst,
			src,
			effect_input_id: Some("Source".into()),
			inputs: Vec::new(),
			values: Vec::new(),
			renderer: None,
			clear_destination: false,
			interactive: false,
		}
	}

	fn window(w: f64, h: f64) -> OfxRectD {
		OfxRectD {
			x1: 0.0,
			y1: 0.0,
			x2: w,
			y2: h,
		}
	}

	fn first_pixel(tex: &Texture) -> [f32; 4] {
		let frame = render::texture_get_frame(tex).unwrap();
		let mut out = [0f32; 4];
		for (i, v) in out.iter_mut().enumerate() {
			*v = f32::from_le_bytes(frame.data[i * 4..i * 4 + 4].try_into().unwrap());
		}
		out
	}

	/// RenderJob::default：全字段的默认值。
	#[test]
	fn render_job_default() {
		let j = RenderJob::default();
		assert_eq!(j.time, 0.0);
		assert!(j.dst.is_dummy());
		assert!(j.src.is_none());
		assert!(j.effect_input_id.is_none());
		assert!(j.inputs.is_empty());
		assert!(j.values.is_empty());
		assert!(j.renderer.is_none());
		assert!(!j.clear_destination);
		assert!(!j.interactive);
	}

	/// read_dst：非 F32 拒绝；0 尺寸 Invalid；正常返回 w/h。
	#[test]
	fn read_dst_validation() {
		let u8tex = u8_texture(2, 2);
		assert!(matches!(read_dst(&u8tex), Err(Error::Failed(_))));

		let params = render::VideoParams {
			format: render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let mut frame = crate::render::Frame::new();
		frame.set_video_params(params);
		let zero = Texture::wrap_frame(frame);
		assert!(matches!(read_dst(&zero), Err(Error::Invalid)));

		let (p, w, h) = read_dst(&f32_texture(3, 2, 0.0)).unwrap();
		assert_eq!((p.width, p.height), (3, 2));
		assert_eq!((w, h), (3.0, 2.0));
	}

	/// pixel_aspect：den == 0 回退 1.0，否则比值。
	#[test]
	fn pixel_aspect_zero_den_falls_back() {
		let mut p = render::VideoParams {
			pixel_aspect_num: 4,
			pixel_aspect_den: 0,
			..Default::default()
		};
		assert_eq!(pixel_aspect(&p), 1.0);
		p.pixel_aspect_num = 3;
		p.pixel_aspect_den = 2;
		assert_eq!(pixel_aspect(&p), 1.5);
	}

	/// plugin_supports_opengl："true"/"needed" 为真；其余/缺失/非字符串
	/// 为假。
	#[test]
	fn plugin_supports_opengl_matrix() {
		let inst = make_instance(vec![], vec![], &[], vec![]);
		assert!(!plugin_supports_opengl(&inst), "缺失属性 → false");

		inst.plugin.descriptor.props.set_one(
			crate::host::PROP_GL_RENDER_SUPPORTED,
			Value::String(cs("true")),
		);
		assert!(plugin_supports_opengl(&inst));
		inst.plugin.descriptor.props.set_one(
			crate::host::PROP_GL_RENDER_SUPPORTED,
			Value::String(cs("needed")),
		);
		assert!(plugin_supports_opengl(&inst));
		inst.plugin.descriptor.props.set_one(
			crate::host::PROP_GL_RENDER_SUPPORTED,
			Value::String(cs("false")),
		);
		assert!(!plugin_supports_opengl(&inst));
		inst.plugin
			.descriptor
			.props
			.set_one(crate::host::PROP_GL_RENDER_SUPPORTED, Value::Int(1));
		assert!(!plugin_supports_opengl(&inst), "非字符串 → false");
	}

	/// pick_input：effect_input_id 命中 src；命中但 src 缺失继续；
	/// inputs 表命中；Source 回退；未知 clip → None。
	#[test]
	fn pick_input_selection_rules() {
		let src = f32_texture(1, 1, 0.25);
		let matte = f32_texture(1, 1, 0.5);

		let mut j = job(f32_texture(1, 1, 0.0), Some(src.clone()));
		assert!(pick_input("Source", &j).is_some());

		// effect_input_id 命中但 src 缺失、inputs 表有同名 → 用表。
		j.src = None;
		j.inputs = vec![("Source".into(), matte.clone())];
		assert!(pick_input("Source", &j).is_some());

		// inputs 表命中非 Source clip。
		j.inputs.push(("Matte".into(), matte.clone()));
		assert!(pick_input("Matte", &j).is_some());

		// effect_input_id 命中但 src 缺失、无 inputs、非 Source → None。
		let j2 = job(f32_texture(1, 1, 0.0), None);
		let mut j2 = j2;
		j2.effect_input_id = Some("Matte".into());
		assert!(pick_input("Matte", &j2).is_none());

		// Source 回退（effect_input_id 不是 Source、无 inputs、src 有）。
		j2.src = Some(src);
		assert!(pick_input("Source", &j2).is_some());

		// 未知 clip → None。
		assert!(pick_input("Nope", &j2).is_none());
	}

	/// begin/endSequenceRender：CPU 路径走非 GL 变体；GL 路径在无 GL
	/// 环境下明确失败（有 GL 环境走 GL 变体）。
	#[test]
	fn begin_end_sequence_cpu_and_gl() {
		let inst = make_instance(vec![], vec![], &[], vec![]);
		let range = OfxRangeD {
			min: 0.0,
			max: 10.0,
		};
		assert!(begin_sequence(&inst, range, None).is_ok());
		assert!(end_sequence(&inst, range, None).is_ok());

		let renderer: Renderer = std::sync::Arc::new(crate::render::GlKindMarker);
		let began = begin_sequence(&inst, range, Some(renderer.clone()));
		if began.is_ok() {
			assert!(end_sequence(&inst, range, Some(renderer)).is_ok());
		} else {
			println!("SKIP: GL 上下文不可用——GL 序列括号分支在无 GL 环境不可达");
			assert!(end_sequence(&inst, range, Some(renderer)).is_err());
		}
	}

	/// render_frame CPU 全链路：输入收集、协商、RoD/RoI、render 与
	/// 输出装配；返回的 RoI 列表与输入 clip 配对（不含 Output）。
	#[test]
	fn render_frame_cpu_happy_path() {
		let inst = make_instance(vec![], vec![], &["Source", "Output"], vec![]);
		let j = job(f32_texture(2, 2, 0.25), Some(f32_texture(2, 2, 0.75)));
		let (out, rois) = render_frame(&inst, &j).expect("CPU render_frame 应成功");
		assert_eq!(rois.len(), 1, "Output clip 不参与 RoI 列表");
		assert_eq!(rois[0].0, "Source");
		// 输入 clip 已连接（property suite 可读）。
		assert!(matches!(
			inst.clips[0].props.get(crate::host::PROP_CLIP_CONNECTED, 0),
			Some(Value::Int(1))
		));
		// 假插件 render 未写像素 → 输出为零填充（Image::allocate 零初始化）。
		assert_eq!(first_pixel(&out), [0.0, 0.0, 0.0, 0.0]);
	}

	/// render_frame：取消标记在任何插件调用前短路。
	#[test]
	fn render_frame_cancel_short_circuits() {
		let inst = make_instance(vec![], vec![], &["Source", "Output"], vec![]);
		inst.cancel
			.store(true, std::sync::atomic::Ordering::Relaxed);
		let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：目标纹理校验失败时在插件调用前返回。
	#[test]
	fn render_frame_rejects_bad_destination() {
		let inst = make_instance(vec![], vec![], &["Source", "Output"], vec![]);
		let j = job(u8_texture(2, 2), None);
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));

		let params = render::VideoParams {
			format: render::PIXEL_FORMAT_F32,
			..Default::default()
		};
		let mut frame = crate::render::Frame::new();
		frame.set_video_params(params);
		let j = job(Texture::wrap_frame(frame), None);
		assert!(matches!(render_frame(&inst, &j), Err(Error::Invalid)));
	}

	/// render_frame：实例无 Output clip → 明确失败。
	#[test]
	fn render_frame_requires_output_clip() {
		let inst = make_instance(vec![], vec![], &["Source"], vec![]);
		let j = job(f32_texture(2, 2, 0.0), None);
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：协商分量 RGBA/RGB/Alpha 与未知值。
	#[test]
	fn render_frame_component_negotiation() {
		for comps in [
			"OfxImageComponentRGB",
			"OfxImageComponentAlpha",
			"OfxImageComponentRGBA",
		] {
			let inst = make_instance(
				vec![],
				vec![("test.components", Value::String(cs(comps)))],
				&["Source", "Output"],
				vec![],
			);
			let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
			render_frame(&inst, &j).expect("已知分量应可渲染");
			let got = inst.clips[1]
				.props
				.get(crate::image::K_IMAGE_EFFECT_PROP_COMPONENTS, 0);
			assert!(
				matches!(&got, Some(Value::String(s)) if s.to_string_lossy() == comps),
				"{comps} 应写回 Output clip"
			);
		}

		// 未知分量 → 协商失败。
		let inst = make_instance(
			vec![],
			vec![("test.components", Value::String(cs("OfxImageComponentBogus")))],
			&["Source", "Output"],
			vec![],
		);
		let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：协商位深 Byte/Short/Half/Float 与未知值。低位深
	/// 输出经 convert_depth 转回 F32 后装配；未知位深在 render 前失败。
	#[test]
	fn render_frame_depth_negotiation() {
		for depth in [
			"OfxBitDepthByte",
			"OfxBitDepthShort",
			"OfxBitDepthHalf",
			"OfxBitDepthFloat",
		] {
			let inst = make_instance(
				vec![],
				vec![("test.depth", Value::String(cs(depth)))],
				&["Source", "Output"],
				vec![],
			);
			let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
			render_frame(&inst, &j).unwrap_or_else(|e| panic!("{depth} 应可渲染：{e}"));
			// 协商结果写回 Output clip（Format 号经 set_video_params 再现）。
			let got = inst.clips[1]
				.props
				.get(crate::image::K_IMAGE_EFFECT_PROP_PIXEL_DEPTH, 0);
			assert!(
				matches!(&got, Some(Value::String(s)) if s.to_string_lossy() == depth),
				"{depth} 应写回 Output clip"
			);
			// 返回纹理恒为 F32（管线工作格式）。
			assert_eq!(
				render::texture_get_params(&j.dst).format,
				render::PIXEL_FORMAT_F32
			);
		}

		let inst = make_instance(
			vec![],
			vec![("test.depth", Value::String(cs("OfxBitDepthBogus")))],
			&["Source", "Output"],
			vec![],
		);
		let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：render action 失败原样上抛。
	#[test]
	fn render_frame_propagates_render_failure() {
		let inst = make_instance(
			vec![],
			vec![("test.render_status", Value::Int(crate::suites::status::FAILED))],
			&["Source", "Output"],
			vec![],
		);
		let j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：isIdentity 命中 → 透传所引输入 clip 的帧；引用
	/// 未知 clip → 明确失败。
	#[test]
	fn render_frame_identity_passthrough() {
		let inst = make_instance(
			vec![],
			vec![
				("test.identity_clip", Value::String(cs("Source"))),
				("test.identity_time", Value::Double(7.0)),
			],
			&["Source", "Output"],
			vec![],
		);
		let src_tex = f32_texture(2, 2, 0.5);
		inst.clips[0].set_input_texture(Some(src_tex.clone()), 0.0);
		let j = job(f32_texture(2, 2, 0.25), Some(src_tex));
		let (out, _) = render_frame(&inst, &j).expect("透传应成功");
		assert_eq!(first_pixel(&out), [0.5, 0.5, 0.5, 0.5]);

		// 引用未知 clip → passthrough 失败。
		let inst = make_instance(
			vec![],
			vec![("test.identity_clip", Value::String(cs("Nope")))],
			&["Source", "Output"],
			vec![],
		);
		let j = job(
			f32_texture(2, 2, 0.25),
			Some(f32_texture(2, 2, 0.5)),
		);
		assert!(matches!(render_frame(&inst, &j), Err(Error::Failed(_))));
	}

	/// render_frame：GL 渲染器 + 插件声明 GL 支持。无 GL 环境回退 CPU
	/// （决策链的 gl 名门失败）；有 GL 环境走真实 GL 路径。
	#[test]
	fn render_frame_gl_decision_and_path() {
		let inst = make_instance(
			vec![(crate::host::PROP_GL_RENDER_SUPPORTED, Value::String(cs("true")))],
			vec![],
			&["Source", "Output"],
			vec![],
		);
		let mut j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		j.renderer = Some(std::sync::Arc::new(crate::render::GlKindMarker));
		if crate::gl_bridge::gl_available() {
			// 真实 GL 路径：回读内容未清屏（未定义像素值）——只断言
			// 装配成功与输出格式/尺寸。
			let (out, _) = render_frame(&inst, &j).expect("GL 路径应成功");
			let params = render::texture_get_params(&out);
			assert_eq!(
				(params.width, params.height, params.format),
				(2, 2, render::PIXEL_FORMAT_F32)
			);
		} else {
			println!("SKIP: GL 不可用（Linux/Windows stub）——仅覆盖 use_opengl 回退决策");
			let (out, _) = render_frame(&inst, &j).expect("GL 不可用应回退 CPU");
			assert_eq!(first_pixel(&out), [0.0, 0.0, 0.0, 0.0]);
		}
	}

	/// render_frame：GL render action 失败 → 回退 CPU（GL 环境下）。
	#[test]
	fn render_frame_gl_failure_falls_back_to_cpu() {
		if !crate::gl_bridge::gl_available() {
			println!("SKIP: GL 不可用——GL 失败回退 CPU 分支不可达");
			return;
		}
		let inst = make_instance(
			vec![(crate::host::PROP_GL_RENDER_SUPPORTED, Value::String(cs("true")))],
			vec![("test.gl_fail", Value::Int(1))],
			&["Source", "Output"],
			vec![],
		);
		let mut j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
		j.renderer = Some(std::sync::Arc::new(crate::render::GlKindMarker));
		let (out, _) = render_frame(&inst, &j).expect("GL 失败应回退 CPU 成功");
		assert_eq!(first_pixel(&out), [0.0, 0.0, 0.0, 0.0]);
	}

	/// render_gl_frame：无 GL 上下文时明确失败（Linux/Windows stub 或
	/// CGL 创建失败）；本机 GL 可用时跳过（该用例只验证失败路径）。
	#[test]
	fn render_gl_frame_reports_unavailable_context() {
		if crate::gl_bridge::gl_available() {
			println!("SKIP: 本机 GL 可用——该用例只验证无 GL 时的明确失败");
			return;
		}
		let inst = make_instance(vec![], vec![], &["Source", "Output"], vec![]);
		let j = job(f32_texture(2, 2, 0.0), None);
		let params = read_dst(&j.dst).unwrap().0;
		let res = render_gl_frame(
			&inst,
			&j,
			RenderScale { x: 1.0, y: 1.0 },
			window(2.0, 2.0),
			2.0,
			2.0,
			&params,
		);
		assert!(res.is_err(), "无 GL 环境 render_gl_frame 应失败");
	}

	/// apply_param_overrides：未知参数名与类型不符跳过；Double 标量
	/// 的 NaN/Inf 清洗与 Min/Max 钳制。
	#[test]
	fn apply_param_overrides_scrubs_and_clamps() {
		let def = ParamDef::new("gain", crate::param::TYPE_DOUBLE);
		def.props
			.set_one(crate::param::P_DEFAULT, Value::Double(0.25));
		def.props.set_one(crate::param::P_MIN, Value::Double(-1.0));
		def.props.set_one(crate::param::P_MAX, Value::Double(1.0));
		let inst = make_instance(
			vec![],
			vec![],
			&[],
			vec![Box::new(ParamInstance::from_def(def))],
		);
		let gain = || inst.params.find("gain").unwrap().get();

		// 未知 key + 类型不符的 key → 都跳过，值不变。
		apply_param_overrides(
			&inst,
			&[
				("nope".into(), crate::node::Value::float(2.0)),
				("gain".into(), crate::node::Value::int(3)),
			],
		);
		assert_eq!(gain(), ParamValue::Double([0.0, 0.0, 0.0], 1));

		// 超上限 → 钳到 max。
		apply_param_overrides(&inst, &[("gain".into(), crate::node::Value::float(5.0))]);
		assert_eq!(gain(), ParamValue::Double([1.0, 0.0, 0.0], 1));

		// 超下限 → 钳到 min。
		apply_param_overrides(&inst, &[("gain".into(), crate::node::Value::float(-5.0))]);
		assert_eq!(gain(), ParamValue::Double([-1.0, 0.0, 0.0], 1));

		// NaN → 回退默认（0.25），再走钳制。
		apply_param_overrides(&inst, &[("gain".into(), crate::node::Value::float(f64::NAN))]);
		assert_eq!(gain(), ParamValue::Double([0.25, 0.0, 0.0], 1));

		// +Inf → 同样回退默认。
		apply_param_overrides(&inst, &[("gain".into(), crate::node::Value::float(f64::INFINITY))]);
		assert_eq!(gain(), ParamValue::Double([0.25, 0.0, 0.0], 1));

		// 多维 Double 不经标量清洗路径：Double2D 直接写入。
		let def2 = ParamDef::new("pos", crate::param::TYPE_DOUBLE2D);
		let inst = make_instance(
			vec![],
			vec![],
			&[],
			vec![Box::new(ParamInstance::from_def(def2))],
		);
		apply_param_overrides(
			&inst,
			&[("pos".into(), crate::node::Value::vec(&[1.0, 2.0]))],
		);
		assert_eq!(
			inst.params.find("pos").unwrap().get(),
			ParamValue::Double([1.0, 2.0, 0.0], 2)
		);
	}

	/// prop_double：Double/Int 提升/缺失默认 0。
	#[test]
	fn prop_double_reads_double_int_and_default() {
		let set = PropertySet::new();
		set.set_one("d", Value::Double(2.5));
		set.set_one("i", Value::Int(3));
		set.set_one("s", Value::String(cs("x")));
		assert_eq!(prop_double(&set, "d", 0), 2.5);
		assert_eq!(prop_double(&set, "i", 0), 3.0);
		assert_eq!(prop_double(&set, "s", 0), 0.0);
		assert_eq!(prop_double(&set, "missing", 0), 0.0);
	}

	/// write_output_frame：非 F32/尺寸不符错误；CPU 就地写回；GPU 经
	/// upload 回写；Planar 在 readback 阶段即失败（Planar 分支防御性
	/// 不可达，见测试注释）。
	#[test]
	fn write_output_frame_validation_and_writeback() {
		let image = f32_image(2, 2, 0.5);

		let mut u8dst = u8_texture(2, 2);
		assert!(matches!(
			write_output_frame(&mut u8dst, &image),
			Err(Error::Failed(_))
		));

		let mut baddst = f32_texture(3, 1, 0.0);
		assert!(matches!(
			write_output_frame(&mut baddst, &image),
			Err(Error::Failed(_))
		));

		let mut cpudst = f32_texture(2, 2, 0.0);
		write_output_frame(&mut cpudst, &image).unwrap();
		let frame = render::texture_get_frame(&cpudst).unwrap();
		assert_eq!(frame.data.as_slice(), image.pixels());

		// GPU：download 帧命中尺寸校验后经 upload 回写。
		let ctx = std::sync::Arc::new(FakeUpload {
			uploads: std::sync::Mutex::new(Vec::new()),
			fail: false,
		});
		let mut gpudst = Texture::gpu(ctx.clone(), 11, 2, 2, oak_core::PixelFormat::F32);
		write_output_frame(&mut gpudst, &image).unwrap();
		{
			let uploads = ctx.uploads.lock().unwrap();
			assert_eq!(uploads.len(), 1);
			assert_eq!(uploads[0].0, 11);
			assert_eq!(uploads[0].1.data.as_slice(), image.pixels());
		}

		// 上传失败 → Failed。
		let ctx = std::sync::Arc::new(FakeUpload {
			uploads: std::sync::Mutex::new(Vec::new()),
			fail: true,
		});
		let mut gpudst = Texture::gpu(ctx, 12, 2, 2, oak_core::PixelFormat::F32);
		assert!(matches!(
			write_output_frame(&mut gpudst, &image),
			Err(Error::Failed(_))
		));

		// Planar：texture_get_frame 在到达 Planar match 分支前即失败
		// （to_frame 对未解析平面纹理返回 State）——该防御分支当前
		// 不可达，这里锁定"平面目标被拒绝"的行为。
		let planar = Texture::wrap_planar(oak_core::texture::PlanarTexture::new(
			std::sync::Arc::new(FakeUpload {
				uploads: std::sync::Mutex::new(Vec::new()),
				fail: false,
			}),
			oak_core::texture::PlanarFormat::Nv12,
			(2, 2),
			(1, 2),
			oak_core::backend::YuvTransform::bt601_limited(),
			(1, 1),
		));
		let mut planardst = planar;
		assert!(matches!(
			write_output_frame(&mut planardst, &image),
			Err(Error::Failed(_))
		));
	}

	/// zip_rois：跳过 Output、按 clips 顺序配对、RoI 不足时默认零矩形。
	#[test]
	fn zip_rois_pairs_inputs_only() {
		let inst = make_instance(vec![], vec![], &["Source", "Matte", "Output"], vec![]);
		let rect = OfxRectD {
			x1: 1.0,
			y1: 2.0,
			x2: 3.0,
			y2: 4.0,
		};
		let zipped = zip_rois(&inst, &[rect]);
		assert_eq!(zipped.len(), 2, "Output 不计入");
		assert_eq!(zipped[0].0, "Source");
		assert_eq!(zipped[0].1.x2, 3.0);
		assert_eq!(zipped[1].0, "Matte");
		assert_eq!(zipped[1].1.x1, 0.0, "RoI 不足时默认矩形");

		let empty = zip_rois(&inst, &[]);
		assert_eq!(empty.len(), 2);
		assert_eq!(empty[0].1.y2, 0.0);
	}

	/// OAK_OFX_TRACE 开启时 use_opengl 决策与 CPU 输出 dump 的 trace
	/// 分支（打印点覆盖）。串行化并还原环境变量。
	#[test]
	fn trace_branches_in_render_frame() {
		static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
		let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
		let prev = std::env::var_os("OAK_OFX_TRACE");
		std::env::set_var("OAK_OFX_TRACE", "1");
		let body = std::panic::catch_unwind(|| {
			// 插件未声明 GL → use_opengl 决策为 false，但 trace 分支
			// 在 renderer 是 GL 时执行（打印 raw GL prop）。
			let inst = make_instance(vec![], vec![], &["Source", "Output"], vec![]);
			let mut j = job(f32_texture(2, 2, 0.0), Some(f32_texture(2, 2, 0.5)));
			j.renderer = Some(std::sync::Arc::new(crate::render::GlKindMarker));
			render_frame(&inst, &j).expect("CPU 回退应成功");
		});
		match prev {
			Some(v) => std::env::set_var("OAK_OFX_TRACE", v),
			None => std::env::remove_var("OAK_OFX_TRACE"),
		}
		if let Err(p) = body {
			std::panic::resume_unwind(p);
		}
	}

	struct FakeUpload {
		uploads: std::sync::Mutex<Vec<(u64, crate::render::Frame)>>,
		fail: bool,
	}

	impl oak_core::backend::GpuContextLike for FakeUpload {
		fn kind(&self) -> oak_core::backend::BackendKind {
			oak_core::backend::BackendKind::Cpu
		}
		fn destroy_texture(&self, _token: u64) {}
		fn upload(&self, token: u64, frame: &crate::render::Frame) -> oak_render::error::Result<()> {
			if self.fail {
				return Err(oak_render::error::Error::State);
			}
			self.uploads
				.lock()
				.unwrap_or_else(|e| e.into_inner())
				.push((token, frame.clone()));
			Ok(())
		}
		fn download(&self, _token: u64) -> oak_render::error::Result<crate::render::Frame> {
			Ok(f32_frame(2, 2, 0.0))
		}
		fn blit(
			&self,
			_src: u64,
			_dst: u64,
			_processor: Option<&oak_core::color::ColorProcessor>,
		) -> oak_render::error::Result<()> {
			Ok(())
		}
	}
}
