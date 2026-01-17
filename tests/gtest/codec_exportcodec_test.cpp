#include <gtest/gtest.h>

#include "codec/exportcodec.h"

TEST(CodecExportCodec, NamesAndFlags)
{
	using olive::ExportCodec;

	EXPECT_EQ(ExportCodec::GetCodecName(ExportCodec::kCodecH264),
			  QStringLiteral("H.264"));
	EXPECT_EQ(ExportCodec::GetCodecName(ExportCodec::kCodecCount),
			  QStringLiteral("Unknown"));

	EXPECT_TRUE(ExportCodec::IsCodecAStillImage(ExportCodec::kCodecPNG));
	EXPECT_FALSE(ExportCodec::IsCodecAStillImage(ExportCodec::kCodecH264));

	EXPECT_TRUE(ExportCodec::IsCodecLossless(ExportCodec::kCodecPCM));
	EXPECT_TRUE(ExportCodec::IsCodecLossless(ExportCodec::kCodecFLAC));
	EXPECT_FALSE(ExportCodec::IsCodecLossless(ExportCodec::kCodecH265));
}
