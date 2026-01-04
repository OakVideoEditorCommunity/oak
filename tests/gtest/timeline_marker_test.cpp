#include <gtest/gtest.h>

#include <QBuffer>
#include <QXmlStreamReader>
#include <QXmlStreamWriter>

#include "timeline/timelinemarker.h"

TEST(TimelineMarker, SaveLoadRoundTrip)
{
	olive::TimelineMarker marker;
	marker.set_time(olive::core::rational(10, 1));
	marker.set_name(QStringLiteral("Marker"));
	marker.set_color(5);

	QByteArray xml;
	QBuffer buffer(&xml);
	buffer.open(QIODevice::WriteOnly);
	QXmlStreamWriter writer(&buffer);
	writer.writeStartDocument();
	writer.writeStartElement(QStringLiteral("marker"));
	marker.save(&writer);
	writer.writeEndElement();
	writer.writeEndDocument();
	buffer.close();

	olive::TimelineMarker loaded;
	QBuffer read_buffer(&xml);
	read_buffer.open(QIODevice::ReadOnly);
	QXmlStreamReader reader(&read_buffer);
	EXPECT_TRUE(reader.readNextStartElement());
	EXPECT_EQ(reader.name().toString(), QStringLiteral("marker"));
	loaded.load(&reader);

	EXPECT_EQ(loaded.time().in(), olive::core::rational(10, 1));
	EXPECT_EQ(loaded.name(), QStringLiteral("Marker"));
	EXPECT_EQ(loaded.color(), 5);
}
