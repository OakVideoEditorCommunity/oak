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
#include "render/texture.h"
#include <algorithm>
#include <cmath>
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
#include "ofxGPURender.h"
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

static olive::AVFramePtr ReadbackTextureToFrame(olive::TexturePtr texture,
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
			const int linesize_pixels =
				olive::plugin::detail::BytesToPixels(frame->linesize[0],
													 params);
			texture->renderer()->DownloadFromTexture(
				texture->id(), params, frame->data[0], linesize_pixels);
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
		const int linesize_pixels =
			olive::plugin::detail::BytesToPixels(rgba_frame->linesize[0],
												 rgba_params);
		texture->renderer()->DownloadFromTexture(
			texture->id(), rgba_params, rgba_frame->data[0], linesize_pixels);
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

int olive::plugin::detail::BytesToPixels(int byte_linesize,
										 const olive::VideoParams &params)
{
	const int bytes_per_pixel =
		olive::VideoParams::GetBytesPerPixel(params.format(),
											 params.channel_count());
	if (byte_linesize <= 0 || bytes_per_pixel <= 0) {
		return 0;
	}
	return byte_linesize / bytes_per_pixel;
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

	auto float_channels = [](AVPixelFormat fmt) -> int {
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
	};

	auto dst_packed_info = [](AVPixelFormat fmt, int *channels,
							  int *bytes_per_component) -> bool {
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
	};

	auto float_dst_from_packed = [&](const olive::AVFramePtr &packed_src,
									 AVPixelFormat float_fmt,
									 const olive::AVFramePtr &float_dst) -> bool {
		if (!packed_src || !float_dst || !packed_src->data[0] ||
			!float_dst->data[0]) {
			return false;
		}
		const int dst_channels = float_channels(float_fmt);
		if (dst_channels == 0) {
			return false;
		}
		int src_channels = 0;
		int bytes_per_component = 0;
		if (!dst_packed_info(static_cast<AVPixelFormat>(packed_src->format),
							 &src_channels, &bytes_per_component)) {
			return false;
		}
		const float inv_scale =
			(bytes_per_component == 2) ? (1.0f / 65535.0f)
									   : (1.0f / 255.0f);
		for (int y = 0; y < packed_src->height; ++y) {
			const uint8_t *src_row =
				packed_src->data[0] + y * packed_src->linesize[0];
			float *dst_row = reinterpret_cast<float *>(
				float_dst->data[0] + y * float_dst->linesize[0]);
			if (bytes_per_component == 2) {
				const uint16_t *src_u16 =
					reinterpret_cast<const uint16_t *>(src_row);
				for (int x = 0; x < packed_src->width; ++x) {
					const uint16_t *pix = src_u16 + x * src_channels;
					float r = pix[0] * inv_scale;
					float g = (src_channels > 1) ? pix[1] * inv_scale : r;
					float b = (src_channels > 2) ? pix[2] * inv_scale : r;
					float a = (src_channels > 3) ? pix[3] * inv_scale : 1.0f;
					dst_row[x * dst_channels + 0] = r;
					if (dst_channels > 1) {
						dst_row[x * dst_channels + 1] = g;
					}
					if (dst_channels > 2) {
						dst_row[x * dst_channels + 2] = b;
					}
					if (dst_channels > 3) {
						dst_row[x * dst_channels + 3] = a;
					}
				}
			} else {
				for (int x = 0; x < packed_src->width; ++x) {
					const uint8_t *pix = src_row + x * src_channels;
					float r = pix[0] * inv_scale;
					float g = (src_channels > 1) ? pix[1] * inv_scale : r;
					float b = (src_channels > 2) ? pix[2] * inv_scale : r;
					float a = (src_channels > 3) ? pix[3] * inv_scale : 1.0f;
					dst_row[x * dst_channels + 0] = r;
					if (dst_channels > 1) {
						dst_row[x * dst_channels + 1] = g;
					}
					if (dst_channels > 2) {
						dst_row[x * dst_channels + 2] = b;
					}
					if (dst_channels > 3) {
						dst_row[x * dst_channels + 3] = a;
					}
				}
			}
		}
		return true;
	};

	const int dst_float_channels = float_channels(dst_fmt);
	if (dst_float_channels > 0) {
		int src_channels = 0;
		int bytes_per_component = 0;
		if (dst_packed_info(static_cast<AVPixelFormat>(src->format),
							&src_channels, &bytes_per_component)) {
			if (float_dst_from_packed(src, dst_fmt, dst)) {
				return dst;
			}
		} else {
			olive::AVFramePtr packed = olive::CreateAVFramePtr();
			AVPixelFormat packed_fmt = (dst_float_channels == 4)
										   ? AV_PIX_FMT_RGBA
										   : (dst_float_channels == 3)
												 ? AV_PIX_FMT_RGB24
												 : AV_PIX_FMT_GRAY8;
			packed->format = packed_fmt;
			packed->width = dst->width;
			packed->height = dst->height;
			if (av_frame_get_buffer(packed.get(), 0) >= 0) {
				SwsContext *pre_ctx = sws_getContext(
					src->width, src->height,
					static_cast<AVPixelFormat>(src->format),
					packed->width, packed->height, packed_fmt, SWS_POINT,
					nullptr, nullptr, nullptr);
				if (pre_ctx) {
					sws_scale(pre_ctx, src->data, src->linesize, 0, src->height,
							  packed->data, packed->linesize);
					sws_freeContext(pre_ctx);
					if (float_dst_from_packed(packed, dst_fmt, dst)) {
						return dst;
					}
				}
			}
		}
	}

	auto clamp01 = [](float v) -> float {
		return std::clamp(v, 0.0f, 1.0f);
	};

	const int src_float_channels = float_channels(
		static_cast<AVPixelFormat>(src->format));
	if (src_float_channels > 0) {
		int dst_channels = 0;
		int bytes_per_component = 0;
		if (dst_packed_info(dst_fmt, &dst_channels, &bytes_per_component) &&
			src->data[0] && dst->data[0]) {
			for (int y = 0; y < src->height; ++y) {
				const float *src_row = reinterpret_cast<const float *>(
					src->data[0] + y * src->linesize[0]);
				uint8_t *dst_row = dst->data[0] + y * dst->linesize[0];
				if (bytes_per_component == 2) {
					auto *dst_row_u16 =
						reinterpret_cast<uint16_t *>(dst_row);
					for (int x = 0; x < src->width; ++x) {
						const float *pix =
							src_row + x * src_float_channels;
						float r = pix[0];
						float g = (src_float_channels > 1) ? pix[1] : r;
						float b = (src_float_channels > 2) ? pix[2] : r;
						float a = (src_float_channels > 3) ? pix[3] : 1.0f;
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
						const float *pix =
							src_row + x * src_float_channels;
						float r = pix[0];
						float g = (src_float_channels > 1) ? pix[1] : r;
						float b = (src_float_channels > 2) ? pix[2] : r;
						float a = (src_float_channels > 3) ? pix[3] : 1.0f;
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

static olive::TexturePtr ConvertTextureForParams(olive::TexturePtr src,
												 const olive::VideoParams &dst_params)
{
	if (!src) {
		return nullptr;
	}
	const olive::VideoParams &src_params = src->params();
	if (src_params.format() == dst_params.format() &&
		src_params.channel_count() == dst_params.channel_count() &&
		src_params.width() == dst_params.width() &&
		src_params.height() == dst_params.height()) {
		return src;
	}

	olive::AVFramePtr frame = src->frame();
	if (!frame || !frame->data[0]) {
		frame = ReadbackTextureToFrame(src, src_params);
	}
	if (!frame || !frame->data[0]) {
		return src;
	}

	olive::AVFramePtr converted = ConvertFrameIfNeeded(frame, dst_params);
	if (!converted || !converted->data[0]) {
		return src;
	}

	auto dst = std::make_shared<olive::Texture>(dst_params);
	int linesize_pixels = LinesizeToPixels(dst_params, converted->linesize[0]);
	if (linesize_pixels <= 0) {
		linesize_pixels = dst_params.effective_width();
	}
	dst->Upload(converted->data[0], linesize_pixels);
	return dst;
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

static void MarkRenderFailure(olive::TexturePtr destination)
{
	if (destination && destination->renderer()) {
		destination->renderer()->ClearDestination(destination.get(), 1.0, 0.0, 1.0, 1.0);
	}
}

void olive::plugin::PluginRenderer::RenderPlugin(TexturePtr src, olive::plugin::PluginJob& job,
					  olive::TexturePtr destination,
					  olive::VideoParams destination_params,
					  bool clear_destination, bool interactive)
{
	auto instance=job.pluginInstance();
	if (!instance) {
		return;
	}
	bool supports_opengl = false;
#ifdef OFX_SUPPORTS_OPENGLRENDER
	const std::string &gl_supported =
		instance->getDescriptor().getProps().getStringProperty(
			kOfxImageEffectPropOpenGLRenderSupported);
	supports_opengl = (gl_supported == "true" || gl_supported == "1");
#endif
	auto *olive_instance =
		dynamic_cast<olive::plugin::OlivePluginInstance *>(instance);
	const bool use_opengl =
		supports_opengl && destination && destination->renderer() &&
		destination->id().isValid();
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
	OfxTime frame = job.time_seconds();

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
			/* 
			input_params.set_width(entry.second->params().width());
			input_params.set_height(entry.second->params().height());
			input_params.set_depth(entry.second->params().depth());
			input_params.set_pixel_aspect_ratio(
				entry.second->params().pixel_aspect_ratio());
			input_params.set_interlacing(entry.second->params().interlacing());
			input_params.set_premultiplied_alpha(
				entry.second->params().premultiplied_alpha());
			input_params.set_divider(entry.second->params().divider());*/
			input_clip->setParams(input_params);
		}
		olive::TexturePtr converted =
			ConvertTextureForParams(entry.second, input_params);
		input_clip->setInputTexture(converted, frame);
	}

	clip->setRegionOfDefinition(regionOfDefinition, frame);
	clip->setOutputTexture(destination, frame);

	stat = instance->beginRenderAction(frame, numFramesToRender,
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

	if (!output_params.is_valid()) {
		qWarning().noquote()
			<< "OFX render skipped due to invalid output params for plugin="
			<< PluginIdForInstance(instance);
		MarkRenderFailure(destination);
		instance->endRenderAction(frame, numFramesToRender, 1.0, interactive,
								  renderScale, true, interactive);
		return;
	}

	// render a frame
	const char *render_field = GetRenderFieldForParams(destination_params);
	stat = instance->renderAction(frame, render_field, renderWindow, renderScale,
								  true, interactive, interactive);
	if (stat != kOfxStatOK && stat != kOfxStatReplyDefault) {
		LogOfxFailure("render", stat, instance);
		MarkRenderFailure(destination);
		instance->endRenderAction(frame, numFramesToRender, 1.0, interactive,
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
			instance->endRenderAction(frame, numFramesToRender, 1.0, interactive,
							  renderScale, true, interactive);
			return;
		}
		AVFramePtr converted = ConvertFrameIfNeeded(frame_ptr, destination_params);
		const AVPixelFormat expected_fmt =
			GetDestinationAVPixelFormat(destination_params);
		destination->handleFrame(converted);
		if (destination->renderer() && converted && converted->data[0] &&
			(expected_fmt == AV_PIX_FMT_NONE ||
			 converted->format == expected_fmt)) {
			int linesize_pixels =
				LinesizeToPixels(destination_params, converted->linesize[0]);
			if (linesize_pixels <= 0) {
				linesize_pixels = destination_params.effective_width();
			}
			destination->Upload(converted->data[0], linesize_pixels);
		} else if (destination->renderer() && converted && converted->data[0]) {
			qWarning().noquote()
				<< "OFX output pixel format mismatch for plugin="
				<< PluginIdForInstance(instance);
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

void olive::plugin::PluginRenderer::AttachOutputTexture(olive::TexturePtr texture)
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
