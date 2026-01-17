#include <gtest/gtest.h>

#include <QOpenGLContext>
#include <QVariant>

#include "render/opengl/openglrenderer.h"

TEST(OpenGLRenderer, DownloadFromTextureWithoutCurrentContext)
{
	QOpenGLContext context;
	ASSERT_TRUE(context.create());
	ASSERT_EQ(QOpenGLContext::currentContext(), nullptr);

	olive::OpenGLRenderer renderer;
	renderer.Init(&context);

	olive::VideoParams params(4, 4, olive::core::PixelFormat::U8, 4,
							  olive::core::rational(1, 1),
							  olive::VideoParams::kInterlaceNone, 1);

	unsigned char buffer[4 * 4 * 4] = {};
	renderer.DownloadFromTexture(QVariant::fromValue<GLuint>(0), params,
								 buffer, 4 * 4);

	EXPECT_EQ(QOpenGLContext::currentContext(), nullptr);
}
