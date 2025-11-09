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

#include "node/project.h"
#include "ofxhImageEffect.h"
#include <cstddef>
#include  <ofxhPluginCache.h>
#include <ofxhBinary.h>

#include <QApplication>
#include <QDir>
#include "OliveHost.h"

#include "OlivePluginInstance.h"
#include "common/Current.h"
using namespace OFX::Host;
using namespace olive::plugin;

void olive::plugin::loadPlugins(QString path)
{
	OFX::Host::ImageEffect::PluginCache imageEffectPluginCache(myHost);
}
OliveHost::~OliveHost()
{
	for(auto& descriptor:descriptors_){
		delete descriptor;
		descriptor=nullptr;
	}
	for(auto& instance:instances_){
		delete instance;
		instance=nullptr;
	}
	
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(ImageEffect::ImageEffectPlugin* plugin)
{
	ImageEffect::Descriptor* desc = new ImageEffect::Descriptor(plugin);
	descriptors_.append(desc);
	return desc;
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(const ImageEffect::Descriptor &rootContext,
							ImageEffect::ImageEffectPlugin *plugin)
{
	ImageEffect::Descriptor* desc =  new ImageEffect::Descriptor(rootContext,plugin);
	descriptors_.append(desc);
	return desc;
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(const std::string &bundlePath,
							ImageEffect::ImageEffectPlugin *plugin)
{
	ImageEffect::Descriptor* desc =  new ImageEffect::Descriptor(bundlePath, plugin);
	descriptors_.append(desc);
	return desc;
}

OFX::Host::ImageEffect::Instance* OliveHost::newInstance(void* clientData,
							OFX::Host::ImageEffect::ImageEffectPlugin* plugin,
							OFX::Host::ImageEffect::Descriptor& desc,
							const std::string& context){
	ImageEffect::Instance* instance=new OlivePluginInstance(plugin,desc,context,Current::getInstance().interactive());
	instances_.append(instance);
	return instance;
};
