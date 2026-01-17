#include <gtest/gtest.h>

#include <QBuffer>
#include <QXmlStreamReader>
#include <QXmlStreamWriter>

#include "node/keyframe.h"
#include "node/value.h"

TEST(NodeKeyframe, SaveLoadRoundTrip)
{
	olive::NodeKeyframe key;
	key.set_input(QStringLiteral("Value"));
	key.set_time(olive::core::rational(1, 24));
	key.set_type(olive::NodeKeyframe::kLinear);
	key.set_value(42.0);
	key.set_bezier_control_in(QPointF(0.1, 0.2));
	key.set_bezier_control_out(QPointF(0.3, 0.4));

	QByteArray xml;
	QBuffer buffer(&xml);
	buffer.open(QIODevice::WriteOnly);
	QXmlStreamWriter writer(&buffer);
	writer.writeStartDocument();
	writer.writeStartElement(QStringLiteral("key"));
	key.save(&writer, olive::NodeValue::kFloat);
	writer.writeEndElement();
	writer.writeEndDocument();
	buffer.close();

	QBuffer read_buffer(&xml);
	read_buffer.open(QIODevice::ReadOnly);
	QXmlStreamReader reader(&read_buffer);
	EXPECT_TRUE(reader.readNextStartElement());
	EXPECT_EQ(reader.name().toString(), QStringLiteral("key"));

	olive::NodeKeyframe loaded;
	EXPECT_TRUE(loaded.load(&reader, olive::NodeValue::kFloat));
	EXPECT_EQ(loaded.input(), QStringLiteral("Value"));
	EXPECT_EQ(loaded.time(), olive::core::rational(1, 24));
	EXPECT_EQ(loaded.type(), olive::NodeKeyframe::kLinear);
	EXPECT_DOUBLE_EQ(loaded.value().toDouble(), 42.0);
	EXPECT_DOUBLE_EQ(loaded.bezier_control_in().x(), 0.1);
	EXPECT_DOUBLE_EQ(loaded.bezier_control_in().y(), 0.2);
	EXPECT_DOUBLE_EQ(loaded.bezier_control_out().x(), 0.3);
	EXPECT_DOUBLE_EQ(loaded.bezier_control_out().y(), 0.4);
}
