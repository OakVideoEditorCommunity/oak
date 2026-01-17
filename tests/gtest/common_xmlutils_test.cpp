#include <gtest/gtest.h>

#include <QBuffer>
#include <QXmlStreamReader>
#include <QXmlStreamWriter>

#include "common/xmlutils.h"

TEST(CommonXmlUtils, ReadNextStartElement)
{
	QByteArray xml = "<root><child>value</child></root>";
	QBuffer buffer(&xml);
	buffer.open(QIODevice::ReadOnly);
	QXmlStreamReader reader(&buffer);

	EXPECT_TRUE(olive::XMLReadNextStartElement(&reader));
	EXPECT_EQ(reader.name().toString(), QStringLiteral("root"));
	EXPECT_TRUE(olive::XMLReadNextStartElement(&reader));
	EXPECT_EQ(reader.name().toString(), QStringLiteral("child"));
}
