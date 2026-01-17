#include <gtest/gtest.h>

#include "widget/timebased/timebasedwidget.h"
#include "node/output/viewer/viewer.h"

TEST(TimeBasedWidget, ConnectViewerNodeNullSafe)
{
	olive::TimeBasedWidget widget(false, false);
	widget.ConnectViewerNode(nullptr);
	EXPECT_EQ(widget.GetConnectedNode(), nullptr);
}

TEST(TimeBasedWidget, ConnectedNodeClearsOnDelete)
{
	olive::TimeBasedWidget widget(false, false);
	auto *viewer = new olive::ViewerOutput();
	widget.ConnectViewerNode(viewer);
	EXPECT_EQ(widget.GetConnectedNode(), viewer);
	delete viewer;
	EXPECT_EQ(widget.GetConnectedNode(), nullptr);
}
