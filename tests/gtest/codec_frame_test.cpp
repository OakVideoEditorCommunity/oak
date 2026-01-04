#include <gtest/gtest.h>

#include "codec/frame.h"

TEST(CodecFrame, DefaultState)
{
	olive::Frame frame;
	EXPECT_EQ(frame.width(), 0);
	EXPECT_EQ(frame.height(), 0);
	EXPECT_EQ(frame.pixel_format(), olive::core::PixelFormat::Format::kFormatNone);
}
