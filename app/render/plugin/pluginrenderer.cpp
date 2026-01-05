/*
 * Oak Video Editor - Non-Linear Video Editor
 * Copyright (C) 2025 Olive CE Team
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 */

//
// Created by mikesolar on 25-10-19.
//
#include <cstdint>
#include <cstring>
#include <qtypes.h>
#include <QDebug>
#define GL_PREAMBLE //QMutexLocker __l(&global_opengl_mutex);
#include "pluginrenderer.h"
#include "pluginSupport/OliveClip.h"
#include "pluginSupport/OlivePluginInstance.h"
#include "common/ffmpegutils.h"
#include "ofxImageEffect.h"
#include "ofxhUtilities.h"
extern "C"{
#include <libavutil/pixfmt.h>
#include <libavutil/pixdesc.h>
#include <libswscale/swscale.h>
}

static AVPixelFormat GetOfxAVPixelFormat(const OFX::Host::ImageEffect::Image &image,
										 int *bytes_per_pixel)
{
	const std::string &depth = image.getStringProperty(kOfxImageEffectPropPixelDepth);
	const std::string &components = image.getStringProperty(kOfxImageEffectPropComponents);

	olive::core::PixelFormat pixel_format = olive::core::PixelFormat::INVALID;
	if (depth == kOfxBitDepthByte) {
		pixel_format = olive::core::PixelFormat::U8;
	} else if (depth == kOfxBitDepthShort) {
		pixel_format = olive::core::PixelFormat::U16;
	} else if (depth == kOfxBitDepthHalf) {
		pixel_format = olive::core::PixelFormat::F16;
	} else if (depth == kOfxBitDepthFloat) {
		pixel_format = olive::core::PixelFormat::F32;
	}

	int channel_count = 0;
	if (components == kOfxImageComponentRGBA) {
		channel_count = 4;
	} else if (components == kOfxImageComponentRGB) {
		channel_count = 3;
	} else if (components == kOfxImageComponentAlpha) {
		channel_count = 1;
	}

	AVPixelFormat pix_fmt = olive::FFmpegUtils::GetFFmpegPixelFormat(pixel_format, channel_count);
	if (pix_fmt == AV_PIX_FMT_NONE && channel_count == 1) {
		if (pixel_format == olive::core::PixelFormat::U8) {
			pix_fmt = AV_PIX_FMT_GRAY8;
		} else if (pixel_format == olive::core::PixelFormat::U16) {
			pix_fmt = AV_PIX_FMT_GRAY16LE;
		} else if (pixel_format == olive::core::PixelFormat::F16) {
			pix_fmt = AV_PIX_FMT_GRAYF16;
		} else if (pixel_format == olive::core::PixelFormat::F32) {
			pix_fmt = AV_PIX_FMT_GRAYF32;
		}
	}

	if (pix_fmt == AV_PIX_FMT_NONE) {
		return AV_PIX_FMT_NONE;
	}

	const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(pix_fmt);
	if (!desc) {
		return AV_PIX_FMT_NONE;
	}

	int bits_per_pixel = av_get_bits_per_pixel(desc);
	if (bits_per_pixel <= 0 || bits_per_pixel % 8 != 0) {
		return AV_PIX_FMT_NONE;
	}

	*bytes_per_pixel = bits_per_pixel / 8;
	return pix_fmt;
}

static AVPixelFormat GetDestinationAVPixelFormat(const olive::VideoParams &params);

static bool ApplyClipPreferencesToParams(
	const OFX::Host::ImageEffect::ClipInstance &clip,
	olive::VideoParams *params)
{
	if (!params) {
		return false;
	}

	olive::core::PixelFormat format = olive::core::PixelFormat::INVALID;
	const std::string &depth = clip.getPixelDepth();
	if (depth == kOfxBitDepthByte) {
		format = olive::core::PixelFormat::U8;
	} else if (depth == kOfxBitDepthShort) {
		format = olive::core::PixelFormat::U16;
	} else if (depth == kOfxBitDepthHalf) {
		format = olive::core::PixelFormat::F16;
	} else if (depth == kOfxBitDepthFloat) {
		format = olive::core::PixelFormat::F32;
	}

	int channels = 0;
	const std::string &components = clip.getComponents();
	if (components == kOfxImageComponentRGBA) {
		channels = 4;
	} else if (components == kOfxImageComponentRGB) {
		channels = 3;
	} else if (components == kOfxImageComponentAlpha) {
		channels = 1;
	}

	if (format == olive::core::PixelFormat::INVALID || channels == 0) {
		return false;
	}

	params->set_format(format);
	params->set_channel_count(channels);
	return true;
}

static olive::AVFramePtr create_avframe_from_ofx_image(OFX::Host::ImageEffect::Image &image)
{
	void *data_ptr = image.getPointerProperty(kOfxImagePropData);
	if (!data_ptr) {
		qWarning().noquote() << "OFX output image missing data pointer";
		return nullptr;
	}

	int bounds[4] = {0, 0, 0, 0};
	image.getIntPropertyN(kOfxImagePropBounds, bounds, 4);
	int width = bounds[2] - bounds[0];
	int height = bounds[3] - bounds[1];
	if (width <= 0 || height <= 0) {
		qWarning().noquote()
			<< "OFX output image has invalid bounds"
			<< bounds[0] << bounds[1] << bounds[2] << bounds[3];
		return nullptr;
	}

	int bytes_per_pixel = 0;
	AVPixelFormat pix_fmt = GetOfxAVPixelFormat(image, &bytes_per_pixel);
	if (pix_fmt == AV_PIX_FMT_NONE || bytes_per_pixel <= 0) {
		qWarning().noquote()
			<< "OFX output image has unsupported pixel format depth="
			<< QString::fromStdString(image.getStringProperty(
				   kOfxImageEffectPropPixelDepth))
			<< "components="
			<< QString::fromStdString(
				   image.getStringProperty(kOfxImageEffectPropComponents));
		return nullptr;
	}

	int row_bytes = image.getIntProperty(kOfxImagePropRowBytes);
	if (row_bytes <= 0) {
		row_bytes = width * bytes_per_pixel;
	}

	uint8_t *src = static_cast<uint8_t *>(data_ptr);
	src += bounds[1] * row_bytes + bounds[0] * bytes_per_pixel;

	olive::AVFramePtr frame = olive::CreateAVFramePtr();
	frame->width = width;
	frame->height = height;
	frame->format = pix_fmt;

	if (av_frame_get_buffer(frame.get(), 0) < 0) {
		return nullptr;
	}

	const int copy_bytes = width * bytes_per_pixel;
	for (int y = 0; y < height; ++y) {
		std::memcpy(frame->data[0] + y * frame->linesize[0],
					src + y * row_bytes,
					copy_bytes);
	}

	return frame;
}

static olive::AVFramePtr create_avframe_from_ofx_image_with_params(
	OFX::Host::ImageEffect::Image &image,
	const olive::VideoParams &params)
{
	void *data_ptr = image.getPointerProperty(kOfxImagePropData);
	if (!data_ptr) {
		return nullptr;
	}

	int bounds[4] = {0, 0, 0, 0};
	image.getIntPropertyN(kOfxImagePropBounds, bounds, 4);
	int width = bounds[2] - bounds[0];
	int height = bounds[3] - bounds[1];
	if (width <= 0 || height <= 0) {
		return nullptr;
	}

	AVPixelFormat pix_fmt = GetDestinationAVPixelFormat(params);
	if (pix_fmt == AV_PIX_FMT_NONE) {
		return nullptr;
	}

	const int bytes_per_pixel =
		params.channel_count() * params.format().byte_count();
	if (bytes_per_pixel <= 0) {
		return nullptr;
	}

	int row_bytes = image.getIntProperty(kOfxImagePropRowBytes);
	if (row_bytes <= 0) {
		row_bytes = width * bytes_per_pixel;
	}

	uint8_t *src = static_cast<uint8_t *>(data_ptr);
	src += bounds[1] * row_bytes + bounds[0] * bytes_per_pixel;

	olive::AVFramePtr frame = olive::CreateAVFramePtr();
	frame->width = width;
	frame->height = height;
	frame->format = pix_fmt;

	if (av_frame_get_buffer(frame.get(), 0) < 0) {
		return nullptr;
	}

	const int copy_bytes = width * bytes_per_pixel;
	for (int y = 0; y < height; ++y) {
		std::memcpy(frame->data[0] + y * frame->linesize[0],
					src + y * row_bytes,
					copy_bytes);
	}

	return frame;
}

static AVPixelFormat GetDestinationAVPixelFormat(const olive::VideoParams &params)
{
	AVPixelFormat pix_fmt =
		olive::FFmpegUtils::GetFFmpegPixelFormat(params.format(),
												 params.channel_count());
	if (pix_fmt == AV_PIX_FMT_NONE && params.channel_count() == 1) {
		if (params.format() == olive::core::PixelFormat::U8) {
			pix_fmt = AV_PIX_FMT_GRAY8;
		} else if (params.format() == olive::core::PixelFormat::U16) {
			pix_fmt = AV_PIX_FMT_GRAY16LE;
		} else if (params.format() == olive::core::PixelFormat::F16) {
			pix_fmt = AV_PIX_FMT_GRAYF16;
		} else if (params.format() == olive::core::PixelFormat::F32) {
			pix_fmt = AV_PIX_FMT_GRAYF32;
		}
	}
	return pix_fmt;
}

static const char *GetRenderFieldForParams(const olive::VideoParams &params)
{
	switch (params.interlacing()) {
	case olive::VideoParams::kInterlaceNone:
		return kOfxImageFieldNone;
	case olive::VideoParams::kInterlacedTopFirst:
	case olive::VideoParams::kInterlacedBottomFirst:
		return kOfxImageFieldBoth;
	}
	return kOfxImageFieldNone;
}

static olive::AVFramePtr ReadbackTextureToFrame(olive::Texture *texture,
												const olive::VideoParams &params)
{
	if (!texture || texture->IsDummy()) {
		return nullptr;
	}

	AVPixelFormat pix_fmt = GetDestinationAVPixelFormat(params);
	if (pix_fmt == AV_PIX_FMT_NONE) {
		return nullptr;
	}

	const AVPixFmtDescriptor *desc = av_pix_fmt_desc_get(pix_fmt);
	if (!desc) {
		return nullptr;
	}

	if (!(desc->flags & AV_PIX_FMT_FLAG_PLANAR)) {
		olive::AVFramePtr frame = olive::CreateAVFramePtr();
		frame->format = pix_fmt;
		frame->width = params.width();
		frame->height = params.height();
		if (av_frame_get_buffer(frame.get(), 0) < 0) {
			return nullptr;
		}

		if (texture->renderer()) {
			texture->renderer()->DownloadFromTexture(
				texture->id(), params, frame->data[0], frame->linesize[0]);
		}
		return frame;
	}

	// Planar formats: read back as RGBA and convert.
	olive::VideoParams rgba_params(
		params.width(), params.height(), olive::core::PixelFormat::U8, 4,
		params.pixel_aspect_ratio(), params.interlacing(), params.divider());

	olive::AVFramePtr rgba_frame = olive::CreateAVFramePtr();
	rgba_frame->format = AV_PIX_FMT_RGBA;
	rgba_frame->width = params.width();
	rgba_frame->height = params.height();
	if (av_frame_get_buffer(rgba_frame.get(), 0) < 0) {
		return nullptr;
	}

	if (texture->renderer()) {
		texture->renderer()->DownloadFromTexture(
			texture->id(), rgba_params, rgba_frame->data[0],
			rgba_frame->linesize[0]);
	}

	olive::AVFramePtr dst = olive::CreateAVFramePtr();
	dst->format = pix_fmt;
	dst->width = params.width();
	dst->height = params.height();
	if (av_frame_get_buffer(dst.get(), 0) < 0) {
		return rgba_frame;
	}

	SwsContext *sws_ctx = sws_getContext(
		rgba_frame->width, rgba_frame->height,
		static_cast<AVPixelFormat>(rgba_frame->format),
		dst->width, dst->height, pix_fmt, SWS_POINT,
		nullptr, nullptr, nullptr);
	if (!sws_ctx) {
		return rgba_frame;
	}

	sws_scale(sws_ctx, rgba_frame->data, rgba_frame->linesize, 0,
			  rgba_frame->height, dst->data, dst->linesize);
	sws_freeContext(sws_ctx);

	return dst;
}

static olive::AVFramePtr ConvertFrameIfNeeded(olive::AVFramePtr src,
											  const olive::VideoParams &dst_params)
{
	if (!src) {
		return nullptr;
	}

	AVPixelFormat dst_fmt = GetDestinationAVPixelFormat(dst_params);
	if (dst_fmt == AV_PIX_FMT_NONE) {
		return src;
	}

	if (src->format == dst_fmt &&
		src->width == dst_params.width() &&
		src->height == dst_params.height()) {
		return src;
	}

	olive::AVFramePtr dst = olive::CreateAVFramePtr();
	dst->format = dst_fmt;
	dst->width = dst_params.width();
	dst->height = dst_params.height();
	if (av_frame_get_buffer(dst.get(), 0) < 0) {
		return src;
	}

	SwsContext *sws_ctx = sws_getContext(
		src->width, src->height, static_cast<AVPixelFormat>(src->format),
		dst->width, dst->height, dst_fmt, SWS_POINT,
		nullptr, nullptr, nullptr);
	if (!sws_ctx) {
		return src;
	}

	sws_scale(sws_ctx, src->data, src->linesize, 0, src->height,
			  dst->data, dst->linesize);
	sws_freeContext(sws_ctx);

	return dst;
}

static int LinesizeToPixels(const olive::VideoParams &params, int linesize_bytes)
{
	const int bytes_per_pixel =
		params.channel_count() * params.format().byte_count();
	if (bytes_per_pixel <= 0) {
		return 0;
	}
	return linesize_bytes / bytes_per_pixel;
}

static QString PluginIdForInstance(const OFX::Host::ImageEffect::Instance *instance)
{
	if (!instance) {
		return QStringLiteral("<null>");
	}
	auto *plugin = instance->getPlugin();
	if (!plugin) {
		return QStringLiteral("<unknown>");
	}
	return QString::fromStdString(plugin->getIdentifier());
}

static void LogOfxFailure(const char *action, OfxStatus stat,
						  const OFX::Host::ImageEffect::Instance *instance)
{
	if (stat == kOfxStatOK || stat == kOfxStatReplyDefault) {
		return;
	}
	qWarning().noquote()
		<< "OFX action failed:" << action
		<< "plugin=" << PluginIdForInstance(instance)
		<< "status=" << OFX::StatStr(stat)
		<< "(" << stat << ")";
}

static void MarkRenderFailure(olive::Texture *destination)
{
	if (destination && destination->renderer()) {
		destination->renderer()->ClearDestination(destination, 1.0, 0.0, 1.0, 1.0);
	}
}

void olive::plugin::PluginRenderer::RenderPlugin(TexturePtr src, olive::plugin::PluginJob& job,
					  olive::Texture *destination,
					  olive::VideoParams destination_params,
					  bool clear_destination, bool interactive)
{
	auto instance=job.pluginInstance();
	if (!instance) {
		return;
	}
	auto *olive_instance =
		dynamic_cast<olive::plugin::OlivePluginInstance *>(instance);
	const bool use_opengl =
		destination && destination->renderer() && destination->id().isValid();
	if (olive_instance) {
		olive_instance->setOpenGLEnabled(use_opengl);
		olive_instance->setVideoParam(destination_params);
	}

	// current render scale of 1
	OfxPointD renderScale;
	renderScale.x = renderScale.y = 1.0;

	// The render window is in pixel coordinates
	// ie: render scale and a PAR of not 1
	OfxRectI  renderWindow;
	renderWindow.x1 = renderWindow.y1 = 0;


	renderWindow.x2 = destination_params.width();
	renderWindow.y2 = destination_params.height();

	/// RoI is in canonical coords,
	OfxRectD  regionOfInterest;
	regionOfInterest.x1 = regionOfInterest.y1 = 0;
	regionOfInterest.x2 = renderWindow.x2 * instance->getProjectPixelAspectRatio();
	regionOfInterest.y2 = renderWindow.y2 * instance->getProjectPixelAspectRatio();

	OfxRectD regionOfDefinition;
	regionOfDefinition.x1 = regionOfDefinition.y1 = 0;
	regionOfDefinition.x2 = destination_params.width();
	regionOfDefinition.y2 = destination_params.height();


	int numFramesToRender=1;

	OliveClipInstance *clip=dynamic_cast<plugin::OliveClipInstance *>(instance->getClip("Output"));
	if (!clip) {
		return;
	}
	clip->setParams(destination_params);

	OfxStatus stat = kOfxStatOK;
	if (olive_instance && !olive_instance->isCreated()) {
		stat = instance->createInstanceAction();
		if(stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
			LogOfxFailure("createInstance", stat, instance);
			MarkRenderFailure(destination);
			return;
		}
	}

	// call get region of interest on each of the inputs
	OfxTime frame = 0;

	const NodeValueRow &values = job.GetValues();
	const auto &clips = instance->getDescriptor().getClips();
	std::map<std::string, TexturePtr> input_textures;
	for (const auto &entry : clips) {
		if (entry.first == kOfxImageEffectOutputClipName) {
			continue;
		}
		OliveClipInstance *input_clip =
			dynamic_cast<OliveClipInstance *>(instance->getClip(entry.first));
		if (!input_clip) {
			continue;
		}
		QString clip_key = QString::fromStdString(entry.first);
		TexturePtr input_tex = values.value(clip_key).toTexture();
		if (!input_tex &&
			entry.first == kOfxImageEffectSimpleSourceClipName) {
			input_tex = values.value(kTextureInput).toTexture();
		}
		if (input_tex) {
			input_textures[entry.first] = input_tex;
			input_clip->setInputTexture(input_tex, frame);
		}
	}

	// now we need to call getClipPreferences on the instance so that it does
	// the clip component/depth logic and caches away the components and depth.
	bool ok = instance->getClipPreferences();
	if (!ok) {
		qWarning().noquote()
			<< "OFX getClipPreferences failed for plugin="
			<< PluginIdForInstance(instance);
		MarkRenderFailure(destination);
		return;
	}

	olive::VideoParams output_params = destination_params;
	if (ApplyClipPreferencesToParams(*clip, &output_params)) {
		clip->setParams(output_params);
	}

	for (const auto &entry : input_textures) {
		OliveClipInstance *input_clip =
			dynamic_cast<OliveClipInstance *>(instance->getClip(entry.first));
		if (!input_clip) {
			continue;
		}
		olive::VideoParams input_params = entry.second->params();
		if (ApplyClipPreferencesToParams(*input_clip, &input_params)) {
			input_clip->setParams(input_params);
		}
		input_clip->setInputTexture(entry.second, frame);
	}

	clip->setRegionOfDefinition(regionOfDefinition, frame);
	clip->setOutputTexture(destination, frame);

	stat = instance->beginRenderAction(0, numFramesToRender,
		1.0, false, renderScale, true,
		interactive);
	if (stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		LogOfxFailure("beginRender", stat, instance);
		MarkRenderFailure(destination);
		return;
	}

#ifdef OFX_SUPPORTS_OPENGLRENDER
	if (use_opengl) {
		instance->contextAttachedAction();
		AttachOutputTexture(destination);
	}
#endif
	// get the RoI for each input clip
	// the regions of interest for each input clip are returned in a std::map
	// on a real host, these will be the regions of each input clip that the
	// effect needs to render a given frame (clipped to the RoD).
	//
	// In our example we are doing full frame fetches regardless.
	std::map<OFX::Host::ImageEffect::ClipInstance *, OfxRectD> rois;
	stat = instance->getRegionOfInterestAction(frame, renderScale,
											   regionOfInterest, rois);
	if (stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		LogOfxFailure("getRegionOfInterest", stat, instance);
		MarkRenderFailure(destination);
		return;
	}

	// render a frame
	const char *render_field = GetRenderFieldForParams(destination_params);
	stat = instance->renderAction(0, render_field, renderWindow, renderScale,
								  true, interactive, interactive);
	if (stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		LogOfxFailure("render", stat, instance);
		MarkRenderFailure(destination);
		instance->endRenderAction(0, numFramesToRender, 1.0, interactive,
								  renderScale, true, interactive);
		return;
	}

	// get the output image buffer (CPU path only)
	std::shared_ptr<OFX::Host::ImageEffect::Image> output_image;
	if (!use_opengl) {
		output_image = clip->getOutputImage(frame);
		if (!output_image) {
			qWarning().noquote()
				<< "OFX getOutputImage returned null for plugin="
				<< PluginIdForInstance(instance);
			MarkRenderFailure(destination);
			instance->endRenderAction(frame, numFramesToRender, 1.0, interactive,
									  renderScale, true, interactive);
			return;
		}
	} else {
		if (!destination || !destination->id().isValid()) {
#ifdef OFX_SUPPORTS_OPENGLRENDER
			DetachOutputTexture();
			instance->contextDetachedAction();
#endif
			instance->endRenderAction(frame, numFramesToRender, 1.0, interactive,
									  renderScale, true, interactive);
			return;
		}
	}

	if (!use_opengl) {
		AVFramePtr frame_ptr = create_avframe_from_ofx_image(*output_image);
		if (!frame_ptr) {
			frame_ptr = create_avframe_from_ofx_image_with_params(
				*output_image, output_params);
		}
		if (!frame_ptr) {
			qWarning().noquote()
				<< "OFX output image conversion failed for plugin="
				<< PluginIdForInstance(instance);
			instance->endRenderAction(0, numFramesToRender, 1.0, interactive,
									  renderScale, true, interactive);
			return;
		}
		AVFramePtr converted = ConvertFrameIfNeeded(frame_ptr, destination_params);
		destination->handleFrame(converted);
		if (destination->renderer() && converted && converted->data[0]) {
			int linesize_pixels =
				LinesizeToPixels(destination_params, converted->linesize[0]);
			if (linesize_pixels <= 0) {
				linesize_pixels = destination_params.effective_width();
			}
			destination->Upload(converted->data[0], linesize_pixels);
		}
	} else {
		AVFramePtr frame_ptr =
			ReadbackTextureToFrame(destination, destination_params);
#ifdef OFX_SUPPORTS_OPENGLRENDER
		DetachOutputTexture();
		instance->contextDetachedAction();
#endif
		if (frame_ptr) {
			AVFramePtr converted =
				ConvertFrameIfNeeded(frame_ptr, destination_params);
			destination->handleFrame(converted);
		}
	}

	instance->endRenderAction(0, numFramesToRender, 1.0, interactive, renderScale, true,interactive
							  );

}

void olive::plugin::PluginRenderer::AttachOutputTexture(olive::Texture *texture)
{
	if (!texture) {
		return;
	}
	AttachTextureAsDestination(texture->id());
}

void olive::plugin::PluginRenderer::DetachOutputTexture()
{
	DetachTextureAsDestination();
}
