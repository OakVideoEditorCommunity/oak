/*
 * Olive Community Edition - Non-Linear Video Editor
 * Copyright (C) 2025 Olive CE Team
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 */

#ifndef PLUGINJOB_H
#define PLUGINJOB_H
#include "acceleratedjob.h"
#include "pluginSupport/OlivePluginInstance.h"

#include <any>
#include <OpenImageIO/detail/fmt/chrono.h>

namespace olive {
namespace plugin {

class PluginJob :public AcceleratedJob{
public:
	explicit PluginJob(OFX::Host::ImageEffect::Instance* pluginInstance, NodeValueRow row): AcceleratedJob()
	{
		this->pluginInstance = pluginInstance;
	}

private:
	OFX::Host::ImageEffect::Instance *pluginInstance=nullptr;

	QHash<OfxTime, QHash<QString, std::any>> paramsOnTime;

	QHash<QString, std::any> params;
};

} // plugin
} // olive

#endif //PLUGINJOB_H
