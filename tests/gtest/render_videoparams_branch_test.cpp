#include <gtest/gtest.h>

extern "C" {
#include <libavutil/avutil.h>
}

#include "render/videoparams.h"

TEST(RenderVideoParams, BytesPerChannelAndPixel)
{
	EXPECT_EQ(olive::VideoParams::GetBytesPerChannel(
				  olive::core::PixelFormat::INVALID),
			  0);
	EXPECT_EQ(olive::VideoParams::GetBytesPerChannel(
				  olive::core::PixelFormat::U8),
			  1);
	EXPECT_EQ(olive::VideoParams::GetBytesPerChannel(
				  olive::core::PixelFormat::U16),
			  2);
	EXPECT_EQ(olive::VideoParams::GetBytesPerChannel(
				  olive::core::PixelFormat::F16),
			  2);
	EXPECT_EQ(olive::VideoParams::GetBytesPerChannel(
				  olive::core::PixelFormat::F32),
			  4);
	EXPECT_EQ(olive::VideoParams::GetBytesPerPixel(
				  olive::core::PixelFormat::U8, 4),
			  4);
}

TEST(RenderVideoParams, DividerAndFormatNames)
{
	EXPECT_EQ(olive::VideoParams::GetNameForDivider(1),
			  QStringLiteral("Full"));
	EXPECT_EQ(olive::VideoParams::GetNameForDivider(3),
			  QStringLiteral("1/3"));

	const QString unknown =
		olive::VideoParams::GetFormatName(olive::core::PixelFormat::INVALID);
	EXPECT_TRUE(unknown.contains(QStringLiteral("Unknown")));
}

TEST(RenderVideoParams, ScalingAndDividerForTarget)
{
	EXPECT_EQ(olive::VideoParams::GetScaledDimension(100, 3), 33);
	EXPECT_EQ(olive::VideoParams::GetDividerForTargetResolution(
				  1920, 1080, 960, 540),
			  2);
	EXPECT_EQ(olive::VideoParams::GetDividerForTargetResolution(
				  1920, 1080, 480, 270),
			  4);
}

TEST(RenderVideoParams, FrameRateStringsAndPixelAspect)
{
	const QString fps =
		olive::VideoParams::FrameRateToString(olive::core::rational(24, 1));
	EXPECT_TRUE(fps.contains(QStringLiteral("24")));
	EXPECT_TRUE(fps.contains(QStringLiteral("FPS")));

	const QStringList names =
		olive::VideoParams::GetStandardPixelAspectRatioNames();
	ASSERT_EQ(names.size(), 6);
	EXPECT_TRUE(names.at(0).contains(QStringLiteral("1.0000")));
}

TEST(RenderVideoParams, ValidityAndTimebase)
{
	olive::VideoParams params;
	EXPECT_FALSE(params.is_valid());
	EXPECT_EQ(params.get_time_in_timebase_units(olive::core::rational(1, 1)),
			  AV_NOPTS_VALUE);

	params.set_width(1920);
	params.set_height(1080);
	params.set_format(olive::core::PixelFormat::U8);
	params.set_channel_count(4);
	params.set_pixel_aspect_ratio(olive::core::rational(1, 1));
	params.set_time_base(olive::core::rational(1, 1));
	params.set_start_time(10);
	EXPECT_TRUE(params.is_valid());
	EXPECT_EQ(params.get_time_in_timebase_units(olive::core::rational(2, 1)),
			  12);
}
