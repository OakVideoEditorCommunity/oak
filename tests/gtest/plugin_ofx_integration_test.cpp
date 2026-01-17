#include <gtest/gtest.h>

#include <cstdlib>

extern "C" {
#include <libavutil/frame.h>
}

#include "common/ffmpegutils.h"
#include "node/value.h"
#include "pluginSupport/OliveHost.h"
#include "pluginSupport/OlivePluginInstance.h"
#include "render/job/pluginjob.h"
#include "render/plugin/pluginrenderer.h"
#include "render/texture.h"
#include "render/videoparams.h"

namespace {

olive::TexturePtr CreateSolidTexture(const olive::VideoParams &params)
{
	olive::AVFramePtr frame = olive::CreateAVFramePtr();
	frame->format = olive::FFmpegUtils::GetFFmpegPixelFormat(
		params.format(), params.channel_count());
	frame->width = params.width();
	frame->height = params.height();
	if (frame->format == AV_PIX_FMT_NONE) {
		return nullptr;
	}
	if (av_frame_get_buffer(frame.get(), 0) < 0) {
		return nullptr;
	}
	if (av_frame_make_writable(frame.get()) < 0) {
		return nullptr;
	}

	const int linesize = frame->linesize[0];
	for (int y = 0; y < frame->height; ++y) {
		std::memset(frame->data[0] + y * linesize, 0x7f, linesize);
	}

	olive::TexturePtr texture = std::make_shared<olive::Texture>(params);
	texture->handleFrame(frame);
	return texture;
}

} // namespace

TEST(PluginIntegration, ChromaKeyerCreateAndRender)
{
	const char *itest = std::getenv("OAK_OFX_ITEST");
	if (!itest || std::string(itest) != "1") {
		GTEST_SKIP() << "OAK_OFX_ITEST not enabled";
	}

	const char *path = std::getenv("OAK_OFX_PLUGIN_PATH");
	if (!path || std::string(path).empty()) {
		GTEST_SKIP() << "OAK_OFX_PLUGIN_PATH not set";
	}

	std::string plugin_id = "net.sf.openfx.ChromaKeyerPlugin";
	if (const char *env_id = std::getenv("OAK_OFX_PLUGIN_ID")) {
		if (*env_id) {
			plugin_id = env_id;
		}
	}

	olive::plugin::loadPlugins(QString::fromUtf8(path));

	auto *cache = OFX::Host::PluginCache::getPluginCache();
	OFX::Host::Plugin *found = nullptr;
	for (auto *plug : cache->getPlugins()) {
		if (plug && plug->getIdentifier() == plugin_id) {
			found = plug;
			break;
		}
	}
	if (!found) {
		GTEST_SKIP() << "Plugin not found: " << plugin_id;
	}

	auto *image_effect =
		dynamic_cast<OFX::Host::ImageEffect::ImageEffectPlugin *>(found);
	ASSERT_TRUE(image_effect);

	const auto &contexts = image_effect->getContexts();
	std::string context = kOfxImageEffectContextFilter;
	if (!contexts.empty() &&
		contexts.find(kOfxImageEffectContextFilter) == contexts.end()) {
		context = *contexts.begin();
	}

	OFX::Host::ImageEffect::Instance *instance =
		image_effect->createInstance(context, nullptr);
	ASSERT_TRUE(instance);

	auto *olive_instance =
		dynamic_cast<olive::plugin::OlivePluginInstance *>(instance);
	ASSERT_TRUE(olive_instance);

	olive::VideoParams params(320, 240, olive::core::PixelFormat::U8, 4);
	olive_instance->setVideoParam(params);

	olive::TexturePtr input = CreateSolidTexture(params);
	ASSERT_TRUE(input);

	olive::NodeValueRow row;
	row.insert(QString::fromStdString(kOfxImageEffectSimpleSourceClipName),
			   olive::NodeValue(olive::NodeValue::kTexture, input));
	row.insert(QStringLiteral("Bg"),
			   olive::NodeValue(olive::NodeValue::kTexture, input));

	olive::plugin::PluginJob job(instance, nullptr, row);
	olive::TexturePtr output = std::make_shared<olive::Texture>(params);

	olive::plugin::PluginRenderer renderer;
	renderer.RenderPlugin(input, job, output, params, true, false);

	EXPECT_TRUE(output->frame());
}
