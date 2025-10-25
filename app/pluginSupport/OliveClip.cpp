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
// Created by mikesolar on 25-10-1.
//

#include "OliveClip.h"

#include "common/Current.h"
const std::string &olive::plugin::OliveClipInstance::getUnmappedBitDepth() const
{
	switch (params_.format()) {
	case PixelFormat::INVALID:
		return kOfxBitDepthNone;
	case PixelFormat::U8:
		return kOfxBitDepthByte;
	case PixelFormat::U16:
		return kOfxBitDepthShort;
	case PixelFormat::F16:
		return kOfxBitDepthHalf;
	case PixelFormat::F32:
		return kOfxBitDepthFloat;
	default:
		return kOfxBitDepthNone;
	}
}
const std::string &
olive::plugin::OliveClipInstance::getUnmappedComponents() const
{
	switch (params_.channel_count()) {
	case 1:
		return kOfxImageComponentAlpha;
	case 3:
		return kOfxImageComponentRGB;
	case 4:
		return kOfxImageComponentRGBA;
	default:
		return kOfxImageComponentNone;
	}
}
const std::string &olive::plugin::OliveClipInstance::getPremult() const
{
	if (params_.premultiplied_alpha()) {
		return kOfxImagePreMultiplied;
	} else {
		return kOfxImageUnPreMultiplied;
	}
}
double olive::plugin::OliveClipInstance::getAspectRatio() const
{
	return params_
		.pixel_aspect_ratio()
		.toDouble();
}
double olive::plugin::OliveClipInstance::getFrameRate() const
{
	return params_.frame_rate().toDouble();
}
void olive::plugin::OliveClipInstance::getFrameRange(double &startFrame,
													 double &endFrame) const
{
	startFrame =
		params_.frame_rate().toDouble() *
		params_.start_time();
	endFrame =
		startFrame +
		params_.frame_rate().toDouble() *
			params_.duration();
}
const std::string &olive::plugin::OliveClipInstance::getFieldOrder() const
{
	switch (params_.interlacing()) {
	case VideoParams::kInterlaceNone:
		return kOfxImageFieldNone;
	case VideoParams::kInterlacedTopFirst:
		return kOfxImageFieldUpper;
	case VideoParams::kInterlacedBottomFirst:
		return kOfxImageFieldLower;
	}
	return kOfxImageFieldNone;
}
bool olive::plugin::OliveClipInstance::getConnected() const
{
	return params_.format() ==
		   PixelFormat::INVALID;
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
	return &image_;
}
OfxRectD
olive::plugin::OliveClipInstance::getRegionOfDefinition(OfxTime time) const
{
	if (regionOfDefinitions_.contains(time)) {
		return regionOfDefinitions_[time];
	}
	OfxRectD regionOfDefinition;
	regionOfDefinition.x1=regionOfDefinition.y1=0;
	regionOfDefinition.x2=params_.width();
	regionOfDefinition.y2=params_.height();
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