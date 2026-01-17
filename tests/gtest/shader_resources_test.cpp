#include <gtest/gtest.h>

#include <QFile>

TEST(Shaders, ResourcesAvailable)
{
	const QStringList shader_paths = {
		QStringLiteral(":/shaders/default.frag"),
		QStringLiteral(":/shaders/default.vert"),
		QStringLiteral(":/shaders/yuv2rgb.frag"),
		QStringLiteral(":/shaders/deinterlace2.frag"),
		QStringLiteral(":/shaders/rgbhistogram.frag"),
		QStringLiteral(":/shaders/rgbhistogram.vert")
	};

	for (const QString &path : shader_paths) {
		QFile file(path);
		ASSERT_TRUE(file.exists()) << "Missing shader resource: "
								   << path.toStdString();
		ASSERT_TRUE(file.open(QIODevice::ReadOnly))
			<< "Failed to open shader resource: " << path.toStdString();
		const QByteArray contents = file.readAll();
		EXPECT_FALSE(contents.isEmpty())
			<< "Shader resource is empty: " << path.toStdString();
	}
}
