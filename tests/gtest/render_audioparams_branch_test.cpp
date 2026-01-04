#include <gtest/gtest.h>

#include "olive/core/render/audioparams.h"

TEST(RenderAudioParams, ValidityAndEquality)
{
	olive::core::AudioParams invalid;
	EXPECT_FALSE(invalid.is_valid());

	olive::core::AudioParams params(
		48000, AV_CH_LAYOUT_STEREO, olive::core::SampleFormat::S16);
	EXPECT_TRUE(params.is_valid());

	olive::core::AudioParams other(
		48000, AV_CH_LAYOUT_STEREO, olive::core::SampleFormat::S16);
	EXPECT_TRUE(params == other);

	other.set_sample_rate(44100);
	EXPECT_TRUE(params != other);
}

TEST(RenderAudioParams, TimeAndSampleConversions)
{
	olive::core::AudioParams params(
		48000, AV_CH_LAYOUT_STEREO, olive::core::SampleFormat::S16);

	EXPECT_EQ(params.channel_count(), 2);
	EXPECT_EQ(params.bytes_per_sample_per_channel(), 2);
	EXPECT_EQ(params.bits_per_sample(), 16);

	EXPECT_EQ(params.time_to_samples(1.0), 48000);
	EXPECT_EQ(params.time_to_bytes_per_channel(1.0), 96000);
	EXPECT_EQ(params.time_to_bytes(1.0), 192000);

	EXPECT_EQ(params.samples_to_bytes(48000), 192000);
	EXPECT_EQ(params.samples_to_bytes_per_channel(48000), 96000);

	EXPECT_EQ(params.bytes_to_samples(192000), 48000);
	EXPECT_EQ(params.bytes_to_time(192000), olive::core::rational(1, 1));
	EXPECT_EQ(params.bytes_per_channel_to_time(96000),
			  olive::core::rational(1, 1));
}
