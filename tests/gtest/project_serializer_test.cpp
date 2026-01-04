#include <gtest/gtest.h>

#include <QBuffer>
#include <QXmlStreamReader>
#include <QXmlStreamWriter>

#include "node/factory.h"
#include "node/project.h"
#include "node/input/time/timeinput.h"
#include "node/project/serializer/serializer.h"

TEST(ProjectSerializer, SaveLoadProjectRoundTrip)
{
	olive::NodeFactory::Initialize();

	olive::Project project;
	project.Initialize();

	auto *node = new olive::TimeInput();
	node->SetLabel(QStringLiteral("TimeInput"));
	node->setParent(&project);

	olive::ProjectSerializer::SaveData save_data(
		olive::ProjectSerializer::kProject, &project, QString());

	QByteArray xml;
	QBuffer buffer(&xml);
	buffer.open(QIODevice::WriteOnly);
	QXmlStreamWriter writer(&buffer);
	EXPECT_EQ(olive::ProjectSerializer::Save(&writer, save_data),
		olive::ProjectSerializer::kSuccess);
	buffer.close();

	olive::Project loaded_project;
	QBuffer read_buffer(&xml);
	read_buffer.open(QIODevice::ReadOnly);
	QXmlStreamReader reader(&read_buffer);
	olive::ProjectSerializer::Result result =
		olive::ProjectSerializer::Load(&loaded_project, &reader,
			olive::ProjectSerializer::kProject);
	EXPECT_EQ(result.code(), olive::ProjectSerializer::kSuccess);
	EXPECT_FALSE(loaded_project.nodes().isEmpty());
}
