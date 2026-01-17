#include <gtest/gtest.h>

#include <QEventLoop>
#include <QTimer>

#include "task/taskmanager.h"

namespace {
class DummyTask final : public olive::Task {
public:
	explicit DummyTask(bool *ran)
		: ran_(ran)
	{
		SetTitle(QStringLiteral("DummyTask"));
	}

protected:
	bool Run() override
	{
		if (ran_) {
			*ran_ = true;
		}
		return true;
	}

private:
	bool *ran_ = nullptr;
};
}

TEST(TaskManager, AddAndRunTask)
{
	olive::TaskManager::CreateInstance();
	olive::TaskManager *mgr = olive::TaskManager::instance();
	ASSERT_NE(mgr, nullptr);

	bool ran = false;
	DummyTask *task = new DummyTask(&ran);

	QEventLoop loop;
	QObject::connect(task, &olive::Task::Finished, &loop, [&loop](olive::Task *, bool) {
		loop.quit();
	});

	mgr->AddTask(task);

	QTimer::singleShot(5000, &loop, &QEventLoop::quit);
	loop.exec();

	EXPECT_TRUE(ran);
	olive::TaskManager::DestroyInstance();
}
