#include <gtest/gtest.h>

#include <QVector2D>
#include <QVector3D>
#include <QVector4D>

#include "node/value.h"

TEST(NodeValue, VectorRoundTrip)
{
	QVector2D v2(1.5f, -2.0f);
	QString encoded = olive::NodeValue::ValueToString(
		olive::NodeValue::kVec2, QVariant::fromValue(v2), false);
	QVariant decoded = olive::NodeValue::StringToValue(
		olive::NodeValue::kVec2, encoded, false);
	QVector2D v2_out = decoded.value<QVector2D>();
	EXPECT_FLOAT_EQ(v2_out.x(), v2.x());
	EXPECT_FLOAT_EQ(v2_out.y(), v2.y());

	QVector3D v3(1.0f, 2.0f, 3.0f);
	encoded = olive::NodeValue::ValueToString(
		olive::NodeValue::kVec3, QVariant::fromValue(v3), false);
	decoded = olive::NodeValue::StringToValue(
		olive::NodeValue::kVec3, encoded, false);
	QVector3D v3_out = decoded.value<QVector3D>();
	EXPECT_FLOAT_EQ(v3_out.x(), v3.x());
	EXPECT_FLOAT_EQ(v3_out.y(), v3.y());
	EXPECT_FLOAT_EQ(v3_out.z(), v3.z());

	QVector4D v4(1.0f, 2.0f, 3.0f, 4.0f);
	encoded = olive::NodeValue::ValueToString(
		olive::NodeValue::kVec4, QVariant::fromValue(v4), false);
	decoded = olive::NodeValue::StringToValue(
		olive::NodeValue::kVec4, encoded, false);
	QVector4D v4_out = decoded.value<QVector4D>();
	EXPECT_FLOAT_EQ(v4_out.x(), v4.x());
	EXPECT_FLOAT_EQ(v4_out.y(), v4.y());
	EXPECT_FLOAT_EQ(v4_out.z(), v4.z());
	EXPECT_FLOAT_EQ(v4_out.w(), v4.w());
}

TEST(NodeValue, BinaryRoundTrip)
{
	QByteArray data("OliveTest");
	QString encoded = olive::NodeValue::ValueToString(
		olive::NodeValue::kBinary, data, false);
	QVariant decoded = olive::NodeValue::StringToValue(
		olive::NodeValue::kBinary, encoded, false);
	EXPECT_EQ(decoded.toByteArray(), data);
}
