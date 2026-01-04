#include <gtest/gtest.h>

#include <QBuffer>
#include <QXmlStreamReader>
#include <QXmlStreamWriter>

#include "node/node.h"
#include "node/serializeddata.h"
#include "node/value.h"

namespace {
class TestNode final : public olive::Node {
public:
	TestNode()
	{
		AddInput(QStringLiteral("Value"), olive::NodeValue::kFloat);
		SetSplitStandardValue(QStringLiteral("Value"), 3.5, -1);
	}

	TestNode *copy() const override
	{
		return new TestNode();
	}

	QString Name() const override
	{
		return QStringLiteral("TestNode");
	}

	QString id() const override
	{
		return QStringLiteral("org.olivevideoeditor.TestNode");
	}

	QVector<CategoryID> Category() const override
	{
		return { kCategoryUnknown };
	}

	QString Description() const override
	{
		return QStringLiteral("Test node for serialization");
	}

	void Value(const NodeValueRow &, const NodeGlobals &, NodeValueTable *) const override
	{
	}
};
}

TEST(NodeSerialization, SaveAndLoadInput)
{
	TestNode node;
	node.SetLabel(QStringLiteral("MyNode"));
	node.SetOverrideColor(2);

	QByteArray xml;
	QBuffer buffer(&xml);
	buffer.open(QIODevice::WriteOnly);
	QXmlStreamWriter writer(&buffer);
	writer.writeStartDocument();
	writer.writeStartElement(QStringLiteral("node"));
	node.Save(&writer);
	writer.writeEndElement();
	writer.writeEndDocument();
	buffer.close();

	TestNode loaded;
	olive::SerializedData data;
	QBuffer read_buffer(&xml);
	read_buffer.open(QIODevice::ReadOnly);
	QXmlStreamReader reader(&read_buffer);
	EXPECT_TRUE(reader.readNextStartElement());
	EXPECT_EQ(reader.name().toString(), QStringLiteral("node"));
	EXPECT_TRUE(loaded.Load(&reader, &data));

	EXPECT_EQ(loaded.GetLabel(), QStringLiteral("MyNode"));
	EXPECT_EQ(loaded.GetOverrideColor(), 2);
	EXPECT_DOUBLE_EQ(loaded.GetSplitStandardValue(QStringLiteral("Value"), -1)
		.first().toDouble(), 3.5);
}
