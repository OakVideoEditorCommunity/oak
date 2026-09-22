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
	}
	Ok(())
}
 			ctx.upload(*token, &frame)
 				.map_err(|e| Error::Failed(format!("输出纹理上传失败：{e}")))?;
 		}
+		// 未解析的平面纹理不会成为插件输出目标（解码路径会先解析）。
+		Texture::Planar(_) => {
