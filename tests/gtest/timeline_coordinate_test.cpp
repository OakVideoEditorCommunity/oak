#include <gtest/gtest.h>

#include "timeline/timelinecoordinate.h"

TEST(TimelineCoordinate, DefaultAndSetters)
{
	olive::TimelineCoordinate coord;
	EXPECT_EQ(coord.GetTrack().type(), olive::Track::kNone);
	EXPECT_EQ(coord.GetTrack().index(), 0);

	const olive::core::rational frame(10, 1);
	olive::Track::Reference ref(olive::Track::kVideo, 2);

	coord.SetFrame(frame);
	coord.SetTrack(ref);

	EXPECT_EQ(coord.GetFrame(), frame);
	EXPECT_EQ(coord.GetTrack(), ref);
}

TEST(TimelineCoordinate, Constructors)
{
	const olive::core::rational frame(5, 1);
	olive::Track::Reference ref(olive::Track::kAudio, 1);

	olive::TimelineCoordinate with_ref(frame, ref);
	EXPECT_EQ(with_ref.GetFrame(), frame);
	EXPECT_EQ(with_ref.GetTrack(), ref);

	olive::TimelineCoordinate with_type(frame, olive::Track::kSubtitle, 3);
	EXPECT_EQ(with_type.GetFrame(), frame);
	EXPECT_EQ(with_type.GetTrack().type(), olive::Track::kSubtitle);
	EXPECT_EQ(with_type.GetTrack().index(), 3);
}
