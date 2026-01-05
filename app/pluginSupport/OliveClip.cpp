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

extern "C" {
#include <libswscale/swscale.h>
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
	return params_.format() == PixelFormat::INVALID;
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
	OfxRectI bounds = rod;
	if (optionalBounds) {
		bounds.x1 = static_cast<int>(std::floor(optionalBounds->x1));
		bounds.y1 = static_cast<int>(std::floor(optionalBounds->y1));
		bounds.x2 = static_cast<int>(std::ceil(optionalBounds->x2));
		bounds.y2 = static_cast<int>(std::ceil(optionalBounds->y2));
	}
	// Clamp bounds to ROD to keep host/plugin coords consistent.
	bounds.x1 = std::max(bounds.x1, rod.x1);
	bounds.y1 = std::max(bounds.y1, rod.y1);
	bounds.x2 = std::min(bounds.x2, rod.x2);
	bounds.y2 = std::min(bounds.y2, rod.y2);

	if (name_ == "Output") {
		if (!images_.contains(time)) {
			// make a new ref counted image
			images_.insert(time, std::make_shared<Image>(*const_cast<OliveClipInstance *>(this),
											 params_, bounds, rod, true));
		}

		// add another reference to the member image for this fetch
		// as we have a ref count of 1 due to construction, this will
		// cause the output image never to delete by the plugin
		// when it releases the image
		images_[time]->addReference();

		images_[time]->EnsureAllocatedFromParams(params_, bounds, rod, true);

		// return it
		return images_[time].get();
	} else {
		if (images_.contains(time)) {
			std::shared_ptr<Image> image = images_.value(time);
			image->EnsureAllocatedFromParams(params_, bounds, rod, false);
			image->addReference();
			return image.get();
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
OfxRectD
olive::plugin::OliveClipInstance::getRegionOfDefinition(OfxTime time) const
{
	if (regionOfDefinitions_.contains(time)) {
		return regionOfDefinitions_[time];
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
	this->params_ = texture->params();
#ifdef OFX_SUPPORTS_OPENGLRENDER
	input_textures_.insert(time, texture);
#endif

	AVFramePtr frame = texture->frame();
	if (!frame || !frame->data[0]) {
		return;
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

	std::shared_ptr<Image> image;
	if (images_.contains(time)) {
		image = images_.value(time);
		image->EnsureAllocatedFromParams(params_, bounds, regionOfDefinition,
										 false);
	} else {
		image = std::make_shared<Image>(*this, params_, bounds,
										regionOfDefinition, false);
		images_.insert(time, image);
	}

	uint8_t *dst = image->data();
	if (!dst) {
		return;
	}

	AVFramePtr src_frame = frame;
	if (frame->format != expected_fmt ||
		frame->width != params_.width() ||
		frame->height != params_.height()) {
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

void olive::plugin::OliveClipInstance::setOutputTexture(Texture *texture,
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

	Texture *gl_texture = nullptr;
	if (isOutput()) {
		gl_texture = output_textures_.value(time, nullptr);
	} else {
		TexturePtr input = input_textures_.value(time);
		gl_texture = input ? input.get() : nullptr;
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
		reinterpret_cast<uintptr_t>(gl_texture)) + "_" +
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
