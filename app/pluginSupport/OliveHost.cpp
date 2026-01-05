/*
 * Oak Video Editor - Non-Linear Video Editor
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
#include <cstdio>
#include <cstring>
#include  <ofxhPluginCache.h>
#include <ofxhBinary.h>

#include <QApplication>
#include <QCoreApplication>
#include <QDir>
#include "OliveHost.h"

#include "OlivePluginInstance.h"
#include "common/Current.h"
#include "ofxMessage.h"
#include <QMessageBox>
using namespace OFX::Host;
using namespace olive::plugin;

namespace olive {
namespace plugin {
class PluginNode;
}
}

namespace {
void AddPluginPath(OFX::Host::PluginCache *cache, const QString &path, bool recurse = true)
{
	if (!cache || path.isEmpty()) {
		return;
	}
	QDir dir(path);
	if (!dir.exists()) {
		return;
	}
	cache->addFileToPath(dir.canonicalPath().toStdString(), recurse);
}

void AddPluginPathsFromEnv(OFX::Host::PluginCache *cache, const char *env_var)
{
	QString raw = qEnvironmentVariable(env_var);
	if (raw.isEmpty()) {
		return;
	}
	const QChar separator = QDir::listSeparator();
	const QStringList paths = raw.split(separator, Qt::SkipEmptyParts);
	for (const QString &path : paths) {
		AddPluginPath(cache, path);
	}
}
}

void olive::plugin::loadPlugins(QString path)
{
	std::shared_ptr<OliveHost> host = std::make_shared<OliveHost>();
	Current::getInstance().setPluginHost(host);

	std::shared_ptr<ImageEffect::PluginCache> imageEffectPluginCache =
		std::make_shared<ImageEffect::PluginCache>(*host);
	Current::getInstance().setPluginCache(imageEffectPluginCache);

	imageEffectPluginCache->registerInCache(
		*OFX::Host::PluginCache::getPluginCache());
	OFX::Host::PluginCache *cache = OFX::Host::PluginCache::getPluginCache();
	cache->setPluginHostPath("Olive");

	const QString home_path = QDir::homePath();
	AddPluginPath(cache, QDir(home_path).filePath(".OFX/Plugins"));
	AddPluginPath(cache, QDir(home_path).filePath(".local/share/OFX/Plugins"));
	AddPluginPath(cache, QDir(home_path).filePath(".local/share/olive/ofx/Plugins"));

	const QString app_dir = QCoreApplication::applicationDirPath();
	AddPluginPath(cache, QDir(app_dir).filePath("../OFX/Plugins"));
	AddPluginPath(cache, QDir(app_dir).filePath("../share/olive/ofx/Plugins"));
	AddPluginPath(cache, QDir(app_dir).filePath("../lib/olive/ofx/Plugins"));

	AddPluginPathsFromEnv(cache, "OLIVE_OFX_PLUGIN_PATH");
	AddPluginPathsFromEnv(cache, "OLIVE_PLUGIN_PATH");

	if (!path.isEmpty()) {
		AddPluginPath(cache, path, true);
	}
	cache->scanPluginFiles();
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

void OliveHost::destroyInstance(OFX::Host::ImageEffect::Instance* instance)
{
	if (!instance) {
		return;
	}
	if (instances_.contains(instance))
	{
		instances_.removeOne(instance);
		delete instance;
	}
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(ImageEffect::ImageEffectPlugin* plugin)
{
	ImageEffect::Descriptor* desc = new ImageEffect::Descriptor(plugin);
#ifdef OFX_SUPPORTS_OPENGLRENDER
	desc->getProps().setStringProperty(kOfxImageEffectPropOpenGLRenderSupported,
									   "true");
#endif
	descriptors_.append(desc);
	return desc;
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(const ImageEffect::Descriptor &rootContext,
							ImageEffect::ImageEffectPlugin *plugin)
{
	ImageEffect::Descriptor* desc =  new ImageEffect::Descriptor(rootContext,plugin);
#ifdef OFX_SUPPORTS_OPENGLRENDER
	desc->getProps().setStringProperty(kOfxImageEffectPropOpenGLRenderSupported,
									   "true");
#endif
	descriptors_.append(desc);
	return desc;
}
ImageEffect::Descriptor *
OliveHost::makeDescriptor(const std::string &bundlePath,
							ImageEffect::ImageEffectPlugin *plugin)
{
	ImageEffect::Descriptor* desc =  new ImageEffect::Descriptor(bundlePath, plugin);
#ifdef OFX_SUPPORTS_OPENGLRENDER
	desc->getProps().setStringProperty(kOfxImageEffectPropOpenGLRenderSupported,
									   "true");
#endif
	descriptors_.append(desc);
	return desc;
}

ImageEffect::Instance* OliveHost::newInstance(void *clientData,
							ImageEffect::ImageEffectPlugin* plugin,
							ImageEffect::Descriptor& desc,
							const std::string& context){
	auto* instance = new OlivePluginInstance(
		plugin, desc, context, Current::getInstance().interactive());
	if (clientData) {
		auto *node = static_cast<PluginNode *>(clientData);
		instance->setNode(
			std::shared_ptr<PluginNode>(node, [](PluginNode *) {}));
	}
	instances_.append(instance);
	return instance;
};
OfxStatus olive::plugin::OliveHost::vmessage(const char *type, const char *id, const char *format,
						   va_list args){
	if (!type || !format) {
		return kOfxStatFailed;
	}

	char buffer[1024];
	buffer[0] = '\0';
	vsnprintf(buffer, sizeof(buffer), format, args);
	QString message(buffer);

	if (strcmp(type, kOfxMessageQuestion) == 0) {
		auto ret = QMessageBox::question(nullptr, "", message,
										 QMessageBox::Ok, QMessageBox::Cancel);
		return (ret == QMessageBox::Ok) ? kOfxStatReplyYes : kOfxStatReplyNo;
	}

	if (strcmp(type, kOfxMessageError) == 0) {
		QMessageBox::critical(nullptr, "", message);
	} else if (strcmp(type, kOfxMessageWarning) == 0) {
		QMessageBox::warning(nullptr, "", message);
	} else {
		QMessageBox::information(nullptr, "", message);
	}

	return kOfxStatOK;
}

OfxStatus olive::plugin::OliveHost::setPersistentMessage(
	const char *type, const char *id, const char *format, va_list args)
{
	if (!type || !format) {
		return kOfxStatFailed;
	}

	char buffer[1024];
	buffer[0] = '\0';
	vsnprintf(buffer, sizeof(buffer), format, args);
	QString message(buffer);

	if (strcmp(type, kOfxMessageError) == 0) {
		persistent_messages_.append({HostMessageType::Error, message});
		QMessageBox::critical(nullptr, "", message);
	} else if (strcmp(type, kOfxMessageWarning) == 0) {
		persistent_messages_.append({HostMessageType::Warning, message});
		QMessageBox::warning(nullptr, "", message);
	} else if (strcmp(type, kOfxMessageMessage) == 0) {
		persistent_messages_.append({HostMessageType::Message, message});
		QMessageBox::information(nullptr, "", message);
	} else {
		return kOfxStatFailed;
	}

	return kOfxStatOK;
}

OfxStatus olive::plugin::OliveHost::clearPersistentMessage()
{
	persistent_messages_.clear();
	return kOfxStatOK;
}
