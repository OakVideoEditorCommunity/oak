#include <gtest/gtest.h>

#include "undo/undostack.h"
#include "undo/undocommand.h"

namespace {
class TestCommand final : public olive::UndoCommand {
public:
	explicit TestCommand(int *value)
		: value_(value)
	{
	}

	olive::Project *GetRelevantProject() const override
	{
		return nullptr;
	}

protected:
	void redo() override
	{
		if (value_) {
			(*value_)++;
		}
	}

	void undo() override
	{
		if (value_) {
			(*value_)--;
		}
	}

private:
	int *value_ = nullptr;
};
}

TEST(UndoStack, PushUndoRedo)
{
	int counter = 0;
	olive::UndoStack stack;
	stack.push(new TestCommand(&counter), QStringLiteral("Test"));
	EXPECT_EQ(counter, 1);
	stack.undo();
	EXPECT_EQ(counter, 0);
	stack.redo();
	EXPECT_EQ(counter, 1);
}
