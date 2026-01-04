/*
 * Olive Community Edition - Non-Linear Video Editor
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
#define GL_PREAMBLE //QMutexLocker __l(&global_opengl_mutex);
#include "pluginrenderer.h"
#include "pluginSupport/OliveClip.h"
#include "pluginSupport/OlivePluginInstance.h"
#include "common/ffmpegutils.h"
#include "ofxImageEffect.h"
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

static olive::AVFramePtr create_avframe_from_ofx_image(OFX::Host::ImageEffect::Image &image)
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

	int bytes_per_pixel = 0;
	AVPixelFormat pix_fmt = GetOfxAVPixelFormat(image, &bytes_per_pixel);
	if (pix_fmt == AV_PIX_FMT_NONE || bytes_per_pixel <= 0) {
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
		}
	}
	return pix_fmt;
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

void olive::plugin::PluginRenderer::RenderPlugin(TexturePtr src, olive::plugin::PluginJob& job,
					  olive::Texture *destination,
					  olive::VideoParams destination_params,
					  bool clear_destination, bool interactive)
{
	auto instance=job.pluginInstance();
	OfxStatus stat;
	stat = instance->createInstanceAction();
	if(stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		return;
	}
	// now we need to to call getClipPreferences on the instance so that it does the clip component/depth
	// logic and caches away the components and depth on each clip.
	bool ok = instance->getClipPreferences();
	if (!ok) {
		return;
	}

	// current render scale of 1
	OfxPointD renderScale;
	renderScale.x = renderScale.y = 1.0;

	// The render window is in pixel coordinates
	// ie: render scale and a PAR of not 1
	OfxRectI  renderWindow;
	renderWindow.x1 = renderWindow.y1 = 0;


	renderWindow.x2 = src->params().width();
	renderWindow.y2 = src->params().height();

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
	stat = instance->beginRenderAction(0, numFramesToRender,
		1.0, false, renderScale, true,
		interactive);
	if (stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		return;
	}

	OliveClipInstance *clip=dynamic_cast<plugin::OliveClipInstance *>(instance->getClip("Output"));
	if (!clip) {
		instance->endRenderAction(0, numFramesToRender, 1.0, interactive, renderScale, true,interactive
								  );
		return;
	}

	// call get region of interest on each of the inputs
	OfxTime frame = 0;

	const NodeValueRow &values = job.GetValues();
	const auto &clips = instance->getDescriptor().getClips();
	for (const auto &entry : clips) {
		if (entry.first == kOfxImageEffectOutputClipName) {
			continue;
		}
		OliveClipInstance *input_clip =
			dynamic_cast<OliveClipInstance *>(instance->getClip(entry.first));
		if (input_clip) {
			QString clip_key = QString::fromStdString(entry.first);
			TexturePtr input_tex = values.value(clip_key).toTexture();
			if (!input_tex &&
				entry.first == kOfxImageEffectSimpleSourceClipName) {
				input_tex = values.value(kTextureInput).toTexture();
			}
			if (input_tex) {
				input_clip->setInputTexture(input_tex, frame);
			}
		}
	}
	// get the RoI for each input clip
	// the regions of interest for each input clip are returned in a std::map
	// on a real host, these will be the regions of each input clip that the
	// effect needs to render a given frame (clipped to the RoD).
	//
	// In our example we are doing full frame fetches regardless.
	std::map<OFX::Host::ImageEffect::ClipInstance *, OfxRectD> rois;
	stat = instance->getRegionOfInterestAction(frame, renderScale,
											   regionOfInterest, rois);
	assert(stat == kOfxStatOK || stat == kOfxStatReplyDefault);

	// render a frame
	stat = instance->renderAction(0,kOfxImageFieldBoth,renderWindow, renderScale, true, interactive, interactive);
	assert(stat == kOfxStatOK);

	// get the output image buffer
	std::shared_ptr<OFX::Host::ImageEffect::Image> output_image = clip->getOutputImage(frame);
	if (!output_image) {
		instance->endRenderAction(frame, numFramesToRender, 1.0, interactive, renderScale, true,interactive
								  );
		return;
	}

	AVFramePtr frame_ptr = create_avframe_from_ofx_image(*output_image);
	if (!frame_ptr) {
		instance->endRenderAction(0, numFramesToRender, 1.0, interactive, renderScale, true,interactive
								  );
		return;
	}

	AVFramePtr converted = ConvertFrameIfNeeded(frame_ptr, destination_params);
	destination->handleFrame(converted);

	instance->endRenderAction(0, numFramesToRender, 1.0, interactive, renderScale, true,interactive
							  );

}
