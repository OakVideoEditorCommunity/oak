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
// Created by mikesolar on 25-10-1.
//

#include "OliveClip.h"

#include "common/Current.h"
#include "common/ffmpegutils.h"
#include "ofxCore.h"
#include "ofxhClip.h"
#include "pluginSupport/image.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <memory>
#ifdef OFX_SUPPORTS_OPENGLRENDER
#include <QOpenGLFunctions>
#endif
#include "common/ffmpegutils.h"
#include "render/renderer.h"
extern "C" {
#include <libswscale/swscale.h>
#include <libavutil/pixdesc.h>
}
namespace {
const std::string kBitDepthNoneStr(kOfxBitDepthNone);
const std::string kBitDepthByteStr(kOfxBitDepthByte);
const std::string kBitDepthShortStr(kOfxBitDepthShort);
const std::string kBitDepthHalfStr(kOfxBitDepthHalf);
const std::string kBitDepthFloatStr(kOfxBitDepthFloat);
const std::string kImageComponentNoneStr(kOfxImageComponentNone);
const std::string kImageComponentAlphaStr(kOfxImageComponentAlpha);
const std::string kImageComponentRGBStr(kOfxImageComponentRGB);
const std::string kImageComponentRGBAStr(kOfxImageComponentRGBA);
const std::string kImagePremultStr(kOfxImagePreMultiplied);
const std::string kImageUnPremultStr(kOfxImageUnPreMultiplied);
const std::string kImageFieldNoneStr(kOfxImageFieldNone);
const std::string kImageFieldUpperStr(kOfxImageFieldUpper);
const std::string kImageFieldLowerStr(kOfxImageFieldLower);

static int BytesToPixels(int byte_linesize, const olive::VideoParams &params)
{
	const int bytes_per_pixel =
		params.channel_count() * params.format().byte_count();
	if (bytes_per_pixel <= 0) {
		return 0;
	}
	return byte_linesize / bytes_per_pixel;
}

static int PackedFloatChannels(AVPixelFormat fmt)
{
	switch (fmt) {
	case AV_PIX_FMT_GRAYF32LE:
	case AV_PIX_FMT_GRAYF32BE:
		return 1;
	case AV_PIX_FMT_RGBF32LE:
	case AV_PIX_FMT_RGBF32BE:
		return 3;
	case AV_PIX_FMT_RGBAF32LE:
	case AV_PIX_FMT_RGBAF32BE:
		return 4;
	default:
		return 0;
	}
}

static bool PackedDstInfo(AVPixelFormat fmt, int *channels,
						  int *bytes_per_component)
{
	switch (fmt) {
	case AV_PIX_FMT_GRAY8:
		*channels = 1;
		*bytes_per_component = 1;
		return true;
	case AV_PIX_FMT_RGB24:
		*channels = 3;
		*bytes_per_component = 1;
		return true;
	case AV_PIX_FMT_RGBA:
		*channels = 4;
		*bytes_per_component = 1;
		return true;
	case AV_PIX_FMT_GRAY16LE:
		*channels = 1;
		*bytes_per_component = 2;
		return true;
	case AV_PIX_FMT_RGB48LE:
		*channels = 3;
		*bytes_per_component = 2;
		return true;
	case AV_PIX_FMT_RGBA64LE:
		*channels = 4;
		*bytes_per_component = 2;
		return true;
	default:
		return false;
	}
}

static olive::AVFramePtr ReadbackTextureToFrame(olive::TexturePtr texture,
												const olive::VideoParams &params)
{
	if (!texture || texture->IsDummy() || !texture->renderer()) {
		return nullptr;
	}

	AVPixelFormat pix_fmt =
		olive::FFmpegUtils::GetFFmpegPixelFormat(params.format(),
										  params.channel_count());
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
		const int linesize_pixels = BytesToPixels(frame->linesize[0], params);
		texture->renderer()->DownloadFromTexture(texture->id(), params,
												 frame->data[0],
												 linesize_pixels);
		return frame;
	}

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

	const int linesize_pixels =
		BytesToPixels(rgba_frame->linesize[0], rgba_params);
	texture->renderer()->DownloadFromTexture(texture->id(), rgba_params,
											 rgba_frame->data[0],
											 linesize_pixels);

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

static olive::AVFramePtr ConvertPackedFloatFrame(olive::AVFramePtr src,
												 AVPixelFormat dst_fmt)
{
	if (!src || !src->data[0]) {
		return nullptr;
	}
	const int src_channels =
		PackedFloatChannels(static_cast<AVPixelFormat>(src->format));
	if (src_channels == 0) {
		return nullptr;
	}

	int dst_channels = 0;
	int bytes_per_component = 0;
	if (!PackedDstInfo(dst_fmt, &dst_channels, &bytes_per_component)) {
		return nullptr;
	}

	olive::AVFramePtr dst = olive::CreateAVFramePtr();
	dst->format = dst_fmt;
	dst->width = src->width;
	dst->height = src->height;
	if (av_frame_get_buffer(dst.get(), 0) < 0) {
		return nullptr;
	}

	auto clamp01 = [](float v) -> float {
		return std::clamp(v, 0.0f, 1.0f);
	};

	for (int y = 0; y < src->height; ++y) {
		const float *src_row = reinterpret_cast<const float *>(
			src->data[0] + y * src->linesize[0]);
		uint8_t *dst_row = dst->data[0] + y * dst->linesize[0];

		if (bytes_per_component == 2) {
			auto *dst_row_u16 = reinterpret_cast<uint16_t *>(dst_row);
			for (int x = 0; x < src->width; ++x) {
				const float *pix = src_row + x * src_channels;
				float r = pix[0];
				float g = (src_channels > 1) ? pix[1] : r;
				float b = (src_channels > 2) ? pix[2] : r;
				float a = (src_channels > 3) ? pix[3] : 1.0f;
				if (dst_channels == 1) {
					float luma = 0.2126f * r + 0.7152f * g + 0.0722f * b;
					dst_row_u16[x] = static_cast<uint16_t>(
						std::lround(clamp01(luma) * 65535.0f));
					continue;
				}
				dst_row_u16[x * dst_channels + 0] =
					static_cast<uint16_t>(
						std::lround(clamp01(r) * 65535.0f));
				dst_row_u16[x * dst_channels + 1] =
					static_cast<uint16_t>(
						std::lround(clamp01(g) * 65535.0f));
				dst_row_u16[x * dst_channels + 2] =
					static_cast<uint16_t>(
						std::lround(clamp01(b) * 65535.0f));
				if (dst_channels == 4) {
					dst_row_u16[x * dst_channels + 3] =
						static_cast<uint16_t>(
							std::lround(clamp01(a) * 65535.0f));
				}
			}
		} else {
			for (int x = 0; x < src->width; ++x) {
				const float *pix = src_row + x * src_channels;
				float r = pix[0];
				float g = (src_channels > 1) ? pix[1] : r;
				float b = (src_channels > 2) ? pix[2] : r;
				float a = (src_channels > 3) ? pix[3] : 1.0f;
				if (dst_channels == 1) {
					float luma = 0.2126f * r + 0.7152f * g + 0.0722f * b;
					dst_row[x] = static_cast<uint8_t>(
						std::lround(clamp01(luma) * 255.0f));
					continue;
				}
				dst_row[x * dst_channels + 0] =
					static_cast<uint8_t>(
						std::lround(clamp01(r) * 255.0f));
				dst_row[x * dst_channels + 1] =
					static_cast<uint8_t>(
						std::lround(clamp01(g) * 255.0f));
				dst_row[x * dst_channels + 2] =
					static_cast<uint8_t>(
						std::lround(clamp01(b) * 255.0f));
				if (dst_channels == 4) {
					dst_row[x * dst_channels + 3] =
						static_cast<uint8_t>(
							std::lround(clamp01(a) * 255.0f));
				}
			}
		}
	}

	return dst;
}
}

const std::string &olive::plugin::OliveClipInstance::getUnmappedBitDepth() const
{
	switch (params_.format()) {
	case PixelFormat::INVALID:
		return kBitDepthNoneStr;
	case PixelFormat::U8:
		return kBitDepthByteStr;
	case PixelFormat::U16:
		return kBitDepthShortStr;
	case PixelFormat::F16:
		return kBitDepthHalfStr;
	case PixelFormat::F32:
		return kBitDepthFloatStr;
	default:
		return kBitDepthNoneStr;
	}
}
const std::string &
olive::plugin::OliveClipInstance::getUnmappedComponents() const
{
	switch (params_.channel_count()) {
	case 1:
		return kImageComponentAlphaStr;
	case 3:
		return kImageComponentRGBStr;
	case 4:
		return kImageComponentRGBAStr;
	default:
		return kImageComponentNoneStr;
	}
}
const std::string &olive::plugin::OliveClipInstance::getPremult() const
{
	if (params_.premultiplied_alpha()) {
		return kImagePremultStr;
	} else {
		return kImageUnPremultStr;
	}
}
double olive::plugin::OliveClipInstance::getAspectRatio() const
{
	return params_.pixel_aspect_ratio().toDouble();
}
double olive::plugin::OliveClipInstance::getFrameRate() const
{
	return params_.frame_rate().toDouble();
}
void olive::plugin::OliveClipInstance::getFrameRange(double &startFrame,
													 double &endFrame) const
{
	startFrame = params_.frame_rate().toDouble() * params_.start_time();
	endFrame =
		startFrame + params_.frame_rate().toDouble() * params_.duration();
}
const std::string &olive::plugin::OliveClipInstance::getFieldOrder() const
{
	switch (params_.interlacing()) {
	case VideoParams::kInterlaceNone:
		return kImageFieldNoneStr;
	case VideoParams::kInterlacedTopFirst:
		return kImageFieldUpperStr;
	case VideoParams::kInterlacedBottomFirst:
		return kImageFieldLowerStr;
	}
	return kImageFieldNoneStr;
}
bool olive::plugin::OliveClipInstance::getConnected() const
{
	if (name_ == kOfxImageEffectOutputClipName) {
#ifdef OFX_SUPPORTS_OPENGLRENDER
		if (!output_textures_.isEmpty()) {
			return true;
		}
#endif
		if(images_.empty())
			return false;
		return true;
	}
#ifdef OFX_SUPPORTS_OPENGLRENDER
	if (!input_textures_.isEmpty()) {
		return true;
	}
#endif
	if(images_.empty())
		return false;
	return true;
}
double olive::plugin::OliveClipInstance::getUnmappedFrameRate() const
{
	return getFrameRate();
}
void olive::plugin::OliveClipInstance::getUnmappedFrameRange(
	double &startFrame, double &endFrame) const
{
	getFrameRange(startFrame, endFrame);
}
bool olive::plugin::OliveClipInstance::getContinuousSamples() const
{
	return false;
}
OFX::Host::ImageEffect::Image *
olive::plugin::OliveClipInstance::getImage(OfxTime time,
										   const OfxRectD *optionalBounds)
{
	OfxRectD rod_d = getRegionOfDefinition(time);
	OfxRectI rod = { static_cast<int>(std::floor(rod_d.x1)),
					 static_cast<int>(std::floor(rod_d.y1)),
					 static_cast<int>(std::ceil(rod_d.x2)),
					 static_cast<int>(std::ceil(rod_d.y2)) };
	(void)optionalBounds;
	// Always return full-frame images to keep input data consistent.
	OfxRectI bounds = rod;

	if (name_ == "Output") {
		if (!images_.contains(time)) {
			// make a new ref counted image
			images_.insert(time, new Image(*const_cast<OliveClipInstance *>(this),
											 params_, bounds, rod, true));
		}

		// add another reference to the member image for this fetch
		// as we have a ref count of 1 due to construction, this will
		// cause the output image never to delete by the plugin
		// when it releases the image
		images_[time]->addReference();

		images_[time]->EnsureAllocatedFromParams(params_, bounds, rod, true);

		// return it
		return images_[time];
	} else {
		if (images_.contains(time)) {
			Image* image = images_.value(time);
			image->EnsureAllocatedFromParams(params_, bounds, rod, false);
			image->addReference();
			return image;
		}

		// Fetch on demand for the input clip.
		// It does get deleted after the plugin is done with it as we
		// have not incremented the auto ref
		//
		// You should do somewhat more sophisticated image management
		// than this.
		Image *image = new Image(*this, params_, bounds, rod, true);
		return image;
	}
}

OFX::Host::ImageEffect::Image*
olive::plugin::OliveClipInstance::getOutputImage(OfxTime time)
{
	if (images_.contains(time)) {
		return images_.value(time);
	}

	OfxRectD rod_d = getRegionOfDefinition(time);
	OfxRectI rod = { static_cast<int>(std::floor(rod_d.x1)),
					 static_cast<int>(std::floor(rod_d.y1)),
					 static_cast<int>(std::ceil(rod_d.x2)),
					 static_cast<int>(std::ceil(rod_d.y2)) };
	OfxRectI bounds = rod;

	auto image = new Image(*this, params_, bounds, rod, true);
	images_.insert(time, image);
	return image;
}
OfxRectD
olive::plugin::OliveClipInstance::getRegionOfDefinition(OfxTime time) const
{
	if (regionOfDefinitions_.contains(time)) {
		return regionOfDefinitions_.value(time);
	}
	OfxRectD regionOfDefinition;
	regionOfDefinition.x1 = regionOfDefinition.y1 = 0;
	regionOfDefinition.x2 = params_.width();
	regionOfDefinition.y2 = params_.height();
	return regionOfDefinition;
}
void olive::plugin::OliveClipInstance::setRegionOfDefinition(
	OfxRectD regionOfDefinition, OfxTime time)
{
	regionOfDefinitions_[time] = regionOfDefinition;
}

void olive::plugin::OliveClipInstance::setDefaultRegionOfDefinition(
	OfxRectD regionOfDefinition)
{
	defaultRegionOfDefinitions_ = regionOfDefinition;
}
void olive::plugin::OliveClipInstance::setInputTexture(TexturePtr texture, OfxTime time){
	if (!texture) {
		return;
	}
	VideoParams incoming = texture->params();
	
	this->params_ = incoming;
#ifdef OFX_SUPPORTS_OPENGLRENDER
	input_textures_.insert(time, texture);
#endif

	AVFramePtr frame = texture->frame();
	if (!frame || !frame->data[0]) {
		frame = ReadbackTextureToFrame(texture, params_);
	}
	AVPixelFormat expected_fmt =
		FFmpegUtils::GetFFmpegPixelFormat(params_.format(),
										  params_.channel_count());
	if (expected_fmt == AV_PIX_FMT_NONE) {
		return;
	}
	OfxRectI bounds = { 0, 0, params_.width(), params_.height() };
	OfxRectD rod_d = getRegionOfDefinition(time);
	OfxRectI regionOfDefinition = { static_cast<int>(std::floor(rod_d.x1)),
									static_cast<int>(std::floor(rod_d.y1)),
									static_cast<int>(std::ceil(rod_d.x2)),
									static_cast<int>(std::ceil(rod_d.y2)) };

	Image* image;
	if (images_.contains(time)) {
		image = images_.value(time);
		image->EnsureAllocatedFromParams(params_, bounds, regionOfDefinition,
										 false);
	} else {
		image = new Image(*this, params_, bounds,
										regionOfDefinition, false);
		images_.insert(time, image);
	}

	uint8_t *dst = (uint8_t*)image->data();
	if (!dst) {
		return;
	}

	if (!frame || !frame->data[0]) {
		std::memset(dst, 0, image->row_bytes() * image->height());
		return;
	}

	AVFramePtr src_frame = frame;
	if (frame->format != expected_fmt ||
		frame->width != params_.width() ||
		frame->height != params_.height()) {
		if (PackedFloatChannels(static_cast<AVPixelFormat>(frame->format)) > 0) {
			AVFramePtr converted =
				ConvertPackedFloatFrame(frame, expected_fmt);
			if (converted) {
				src_frame = converted;
				goto copy_pixels;
			}
		}
		AVFramePtr converted = CreateAVFramePtr();
		converted->format = expected_fmt;
		converted->width = params_.width();
		converted->height = params_.height();
		if (av_frame_get_buffer(converted.get(), 0) < 0) {
			return;
		}

		SwsContext *sws_ctx = sws_getContext(
			frame->width, frame->height,
			static_cast<AVPixelFormat>(frame->format),
			converted->width, converted->height,
			static_cast<AVPixelFormat>(converted->format),
			SWS_POINT, nullptr, nullptr, nullptr);
		if (!sws_ctx) {
			return;
		}

		sws_scale(sws_ctx, frame->data, frame->linesize, 0, frame->height,
				  converted->data, converted->linesize);
		sws_freeContext(sws_ctx);

		src_frame = converted;
	}

copy_pixels:
	int bytes_per_component = params_.format().byte_count();
	int bytes_per_row = params_.width() * params_.channel_count() *
						bytes_per_component;
	int src_row_bytes = src_frame->linesize[0];
	int dst_row_bytes = image->row_bytes();
	int copy_bytes = std::min(bytes_per_row,
							  std::min(src_row_bytes, dst_row_bytes));
	int copy_height = std::min(image->height(), src_frame->height);

	const uint8_t *src = src_frame->data[0];
	for (int y = 0; y < copy_height; ++y) {
		std::memcpy(dst + y * dst_row_bytes, src + y * src_row_bytes,
					copy_bytes);
	}

	
}

void olive::plugin::OliveClipInstance::setOutputTexture(TexturePtr texture,
														OfxTime time)
{
#ifdef OFX_SUPPORTS_OPENGLRENDER
	if (!texture) {
		return;
	}
	output_textures_.insert(time, texture);
#else
	(void)texture;
	(void)time;
#endif
}

#ifdef OFX_SUPPORTS_OPENGLRENDER
OFX::Host::ImageEffect::Texture *
olive::plugin::OliveClipInstance::loadTexture(OfxTime time, const char *format,
											  const OfxRectD *optionalBounds)
{
	(void)format;

	TexturePtr gl_texture = nullptr;
	if (isOutput()) {
		gl_texture = output_textures_.value(time, nullptr);
	} else {
		TexturePtr input = input_textures_.value(time);
		gl_texture = input ? input : nullptr;
	}

	if (!gl_texture || gl_texture->IsDummy() || !gl_texture->id().isValid()) {
		return nullptr;
	}

	OfxRectD rod_d = getRegionOfDefinition(time);
	OfxRectI rod = { static_cast<int>(std::floor(rod_d.x1)),
					 static_cast<int>(std::floor(rod_d.y1)),
					 static_cast<int>(std::ceil(rod_d.x2)),
					 static_cast<int>(std::ceil(rod_d.y2)) };
	OfxRectI bounds = rod;
	if (optionalBounds) {
		bounds.x1 = static_cast<int>(std::floor(optionalBounds->x1));
		bounds.y1 = static_cast<int>(std::floor(optionalBounds->y1));
		bounds.x2 = static_cast<int>(std::ceil(optionalBounds->x2));
		bounds.y2 = static_cast<int>(std::ceil(optionalBounds->y2));
	}
	bounds.x1 = std::max(bounds.x1, rod.x1);
	bounds.y1 = std::max(bounds.y1, rod.y1);
	bounds.x2 = std::min(bounds.x2, rod.x2);
	bounds.y2 = std::min(bounds.y2, rod.y2);

	const int bytes_per_row =
		params_.width() * params_.channel_count() * params_.format().byte_count();
	const std::string &field = getFieldOrder();
	const std::string unique_id = std::to_string(
		reinterpret_cast<uintptr_t>(gl_texture.get())) + "_" +
		std::to_string(static_cast<long long>(time));

	const int texture_id = gl_texture->id().value<GLuint>();
	OFX::Host::ImageEffect::Texture *texture =
		new OFX::Host::ImageEffect::Texture(
			*this, 1.0, 1.0, texture_id, GL_TEXTURE_2D, bounds, rod,
			bytes_per_row, field, unique_id);
	texture->addReference();
	return texture;
}
#endif
