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
 */
#ifndef OLIVE_HOST_H
#define OLIVE_HOST_H
#include  "ofxhHost.h"
#include "ofxhImageEffectAPI.h"
#include "ofxCore.h"
#include "ofxhImageEffect.h"

#include <QVariant>
#include <cstdint>
#include <QString>
#include <QMap>
#include <any>
#include <list>
#include <OpenImageIO/detail/fmt/chrono.h>
#include <qlist.h>
namespace olive {
namespace plugin {


void loadPlugins(QString path);
class OliveHost: public OFX::Host::ImageEffect::Host{
public:
	OliveHost()=default;
	~OliveHost() override;
	void destroyInstance(OFX::Host::ImageEffect::Instance *instance);

	bool pluginSupported(OFX::Host::ImageEffect::ImageEffectPlugin *plugin,
		std::string &reason) const override
	{
		return true;
	};

	OFX::Host::ImageEffect::Instance* newInstance(void* clientData,
							OFX::Host::ImageEffect::ImageEffectPlugin* plugin,
							OFX::Host::ImageEffect::Descriptor& desc,
							const std::string& context) override;


	OFX::Host::ImageEffect::Descriptor *makeDescriptor(
		OFX::Host::ImageEffect::ImageEffectPlugin* plugin) override;

	OFX::Host::ImageEffect::Descriptor *makeDescriptor(
		const OFX::Host::ImageEffect::Descriptor &rootContext,
		OFX::Host::ImageEffect::ImageEffectPlugin *plugin) override;

	OFX::Host::ImageEffect::Descriptor *makeDescriptor(
		const std::string &bundlePath,
		OFX::Host::ImageEffect::ImageEffectPlugin *plugin) override;

private:
	QList<OFX::Host::ImageEffect::Descriptor*> descriptors_;
	QList<OFX::Host::ImageEffect::Instance*> instances_;
};
}
}
#endif