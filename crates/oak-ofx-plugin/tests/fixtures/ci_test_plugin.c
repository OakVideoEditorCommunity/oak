// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

// Minimal but real OFX image-effect plugin used by CI to prove the host's
// plugin discovery/loading path end to end: the CI step compiles this file
// into a .ofx.bundle, points OFX_PLUGIN_PATH at it and runs the scan_probe
// example, asserting the plugin is discovered and registered.
//
// Only what the host actually exercises is implemented: setHost, the load /
// describe / describeInContext / createInstance / destroyInstance actions
// (registration instantiates every plugin once for its metadata). describe
// declares the filter context through the real property-suite round trip.

#include <string.h>

typedef void* OfxPropertySetHandle;
typedef int OfxStatus;

typedef struct OfxHost {
	OfxPropertySetHandle host;
	const void* (*fetchSuite)(OfxPropertySetHandle host, const char* name, int version);
} OfxHost;

typedef struct OfxPlugin {
	const char* pluginApi;
	int apiVersion;
	const char* pluginIdentifier;
	unsigned int pluginVersionMajor;
	unsigned int pluginVersionMinor;
	void (*setHost)(OfxHost* host);
	OfxStatus (*mainEntry)(const char* action, const void* handle,
		OfxPropertySetHandle inArgs, OfxPropertySetHandle outArgs);
} OfxPlugin;

typedef struct OfxPropertySuiteV1 {
	OfxStatus (*propSetPointer)(OfxPropertySetHandle, const char*, int, void*);
	OfxStatus (*propSetString)(OfxPropertySetHandle, const char*, int, const char*);
	/* The fixture only ever sets strings; the remaining entries stay
	   untouched (the host's table is larger, we just never call them). */
} OfxPropertySuiteV1;

enum {
	kOfxStatOK = 0,
	kOfxStatReplyDefault = 16,
};

static OfxHost* g_host = 0;

static void ci_set_host(OfxHost* host) {
	g_host = host;
}

static OfxStatus ci_main_entry(const char* action, const void* handle,
	OfxPropertySetHandle inArgs, OfxPropertySetHandle outArgs) {
	(void)inArgs;
	(void)outArgs;
	if (strcmp(action, "OfxActionLoad") == 0) {
		return kOfxStatOK;
	}
	if (strcmp(action, "OfxActionDescribe") == 0) {
		if (g_host == 0 || g_host->fetchSuite == 0) {
			return 1; /* kOfxStatFailed: no host, nothing to describe on */
		}
		OfxPropertySuiteV1* props = (OfxPropertySuiteV1*)g_host->fetchSuite(
			g_host->host, "OfxPropertySuite", 1);
		if (props == 0 || props->propSetString == 0) {
			return 1;
		}
		OfxPropertySetHandle desc = (OfxPropertySetHandle)handle;
		if (props->propSetString(desc, "OfxPropLabel", 0, "Oak CI Test Plugin") != kOfxStatOK) {
			return 1;
		}
		if (props->propSetString(desc, "OfxImageEffectPropSupportedContexts", 0,
				"OfxImageEffectContextFilter") != kOfxStatOK) {
			return 1;
		}
		return kOfxStatOK;
	}
	if (strcmp(action, "OfxImageEffectActionDescribeInContext") == 0 ||
		strcmp(action, "OfxActionCreateInstance") == 0 ||
		strcmp(action, "OfxActionDestroyInstance") == 0) {
		return kOfxStatOK;
	}
	return kOfxStatReplyDefault;
}

static OfxPlugin g_plugin = {
	"OfxImageEffectPluginAPI",
	1,
	"rs.oak.CiTestPlugin",
	1,
	0,
	ci_set_host,
	ci_main_entry,
};

int OfxGetNumberOfPlugins(void) {
	return 1;
}

OfxPlugin* OfxGetPlugin(int nth) {
	return nth == 0 ? &g_plugin : (OfxPlugin*)0;
}
