#include <gtest/gtest.h>

#include "ofxImageEffect.h"
#include "ofxhClip.h"
#include "pluginSupport/OliveClip.h"
#include "pluginSupport/image.h"

namespace {
olive::VideoParams MakeParams(int width, int height,
							  olive::core::PixelFormat format,
							  int channels,
							  bool premultiplied)
{
	olive::VideoParams params;
	params.set_width(width);
	params.set_height(height);
	params.set_format(format);
	params.set_channel_count(channels);
	params.set_premultiplied_alpha(premultiplied);
	return params;
}
}

TEST(PluginSupportImage, AllocateFromParamsSetsProperties)
{
	OFX::Host::ImageEffect::ClipDescriptor desc(kOfxImageEffectOutputClipName);
	olive::VideoParams params =
		MakeParams(640, 480, olive::core::PixelFormat::U8, 4, true);
	olive::plugin::OliveClipInstance clip(nullptr, desc, params);

	olive::plugin::Image image(clip);
	OfxRectI bounds = { 0, 0, 640, 480 };
	OfxRectI rod = bounds;
	image.AllocateFromParams(params, bounds, rod, true);

	EXPECT_NE(image.data(), nullptr);
	EXPECT_EQ(image.width(), 640);
	EXPECT_EQ(image.height(), 480);
	EXPECT_EQ(image.row_bytes(), 640 * 4);
	EXPECT_EQ(image.pixel_format(), olive::core::PixelFormat::U8);
	EXPECT_EQ(image.channel_count(), 4);
	EXPECT_TRUE(image.premultiplied_alpha());
}

TEST(PluginSupportImage, EnsureAllocatedFromParamsClearsAndResizes)
{
	OFX::Host::ImageEffect::ClipDescriptor desc(kOfxImageEffectOutputClipName);
	olive::VideoParams params =
		MakeParams(64, 32, olive::core::PixelFormat::U8, 3, false);
	olive::plugin::OliveClipInstance clip(nullptr, desc, params);

	olive::plugin::Image image(clip);
	OfxRectI bounds = { 0, 0, 64, 32 };
	OfxRectI rod = bounds;
	image.AllocateFromParams(params, bounds, rod, true);
	ASSERT_NE(image.data(), nullptr);
	image.data()[0] = 0xAB;

	image.EnsureAllocatedFromParams(params, bounds, rod, true);
	EXPECT_EQ(image.data()[0], 0);

	OfxRectI new_bounds = { 0, 0, 16, 16 };
	image.EnsureAllocatedFromParams(params, new_bounds, rod, false);
	EXPECT_EQ(image.width(), 16);
	EXPECT_EQ(image.height(), 16);
}

TEST(PluginSupportImage, PropertyFallbacks)
{
	OFX::Host::ImageEffect::ClipDescriptor desc(kOfxImageEffectOutputClipName);
	olive::VideoParams params =
		MakeParams(1, 1, olive::core::PixelFormat::INVALID, 0, false);
	olive::plugin::OliveClipInstance clip(nullptr, desc, params);

	olive::plugin::Image image(clip);
	image.setStringProperty(kOfxImageEffectPropPixelDepth, kOfxBitDepthHalf);
	image.setStringProperty(kOfxImageEffectPropComponents,
							kOfxImageComponentRGB);
	image.setStringProperty(kOfxImageEffectPropPreMultiplication,
							kOfxImagePreMultiplied);
	image.setIntProperty(kOfxImagePropBounds, 10, 0);
	image.setIntProperty(kOfxImagePropBounds, 20, 1);
	image.setIntProperty(kOfxImagePropBounds, 42, 2);
	image.setIntProperty(kOfxImagePropBounds, 70, 3);

	EXPECT_EQ(image.pixel_format(), olive::core::PixelFormat::F16);
	EXPECT_EQ(image.channel_count(), 3);
	EXPECT_TRUE(image.premultiplied_alpha());
	EXPECT_EQ(image.width(), 32);
	EXPECT_EQ(image.height(), 50);
}
