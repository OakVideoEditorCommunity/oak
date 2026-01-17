#include <gtest/gtest.h>

#include "config/config.h"

TEST(Config, DefaultsPresent)
{
	olive::Config &cfg = olive::Config::Current();
	cfg.SetDefaults();

	EXPECT_TRUE(cfg[QStringLiteral("Style")].isValid());
	EXPECT_TRUE(cfg[QStringLiteral("TimecodeDisplay")].isValid());
	EXPECT_TRUE(cfg[QStringLiteral("DefaultStillLength")].isValid());
}

TEST(Config, SetAndGetValues)
{
	olive::Config &cfg = olive::Config::Current();
	cfg[QStringLiteral("UnitTestValue")] = 42;
	EXPECT_EQ(cfg[QStringLiteral("UnitTestValue")].toInt(), 42);
}
