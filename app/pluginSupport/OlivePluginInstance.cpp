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
#include "OlivePluginInstance.h"

#include "OliveClip.h"
#include "ofxCore.h"
#include "ofxMessage.h"
#include "common/Current.h"

#include <cstdio>
#include <QMessageBox>
#include <qmessagebox.h>
#include <qobject.h>
#include <string.h>
#include <QString>
#include "paraminstance.h"
namespace olive
{
namespace plugin
{
OfxStatus OlivePluginInstance::vmessage(const char *type, const char *id,
								  const char *format, va_list args)
{
	char *buffer = new char[1024];

	memset(buffer, 0, 1024 * sizeof(char));
	vsprintf(buffer, format, args);

	QString message(buffer);

	delete[] buffer;

	if (strncmp(type, kOfxMessageQuestion, strlen(kOfxMessageQuestion))) {
		auto ret = QMessageBox::information(
			nullptr, "", message, QMessageBox::Ok, QMessageBox::Cancel);

		if (ret == QMessageBox::Ok) {
			return kOfxStatReplyYes;
		} else {
			return kOfxStatReplyNo;
		}

	} else {
		QMessageBox::information(nullptr, "", message);
		return kOfxStatOK;
	}
}
OfxStatus OlivePluginInstance::setPersistentMessage(const char *type, const char *id,
											  const char *format, va_list args)
{
	char *buffer = new char[1024];

	memset(buffer, 0, 1024 * sizeof(char));
	int ret = vsprintf(buffer, format, args);
	if (ret < 0) {
		return kOfxStatFailed;
	}
	QString message(buffer);

	delete[] buffer;

	// If This is a error message
	if (strncmp(type, kOfxMessageError, strlen(kOfxMessageError)) == 0) {
		persistentErrors_.append({ ErrorType::Error, message });
		QMessageBox::critical(nullptr, "", message);
		// TODO: tell the shell to show the error
	}
	// A warning
	else if (strncmp(type, kOfxMessageWarning, strlen(kOfxMessageError)) == 0) {
		persistentErrors_.append({ ErrorType::Warning, message });
		QMessageBox::warning(nullptr, "", message);
		// TODO: tell the shell to show the warning

	}
	// A simple information
	else if (strncmp(type, kOfxMessageMessage, strlen(kOfxMessageError)) == 0) {
		persistentErrors_.append({ ErrorType::Message, message });
		QMessageBox::information(nullptr, "", message);
	} else {
		return kOfxStatFailed;
	}
	if (node_) {
		emit node_->MessageCountChanged();
	}
	return kOfxStatOK;
}
OfxStatus OlivePluginInstance::clearPersistentMessage()
{
	persistentErrors_.clear();
	// TODO: tell the shell to remove message.
	if (node_) {
		emit node_->MessageCountChanged();
	}
	return kOfxStatOK;
}
void OlivePluginInstance::getProjectSize(double &xSize, double &ySize) const
{
	xSize =params_.width();
	ySize =params_.height();
}
void OlivePluginInstance::getProjectOffset(double &xOffset, double &yOffset) const
{
	xOffset =params_.x();
	yOffset =params_.y();
}
void OlivePluginInstance::getProjectExtent(double &xSize, double &ySize) const
{
	xSize =params_.width();
	ySize =params_.height();
	// TODO: Ensure this project does not support this.
}
double OlivePluginInstance::getProjectPixelAspectRatio() const
{
	return Current::getInstance()
		.currentVideoParams()
		.pixel_aspect_ratio()
		.toDouble();
}
double OlivePluginInstance::getFrameRate() const
{
	return params_.frame_rate().toDouble();
}

double OlivePluginInstance::getEffectDuration() const
{
	// Return a default duration value
	return 100.0;
}

double OlivePluginInstance::getFrameRecursive() const
{
	// Return current frame (this would typically be set by the host during rendering)
	return 0.0;
}

void OlivePluginInstance::getRenderScaleRecursive(double &x, double &y) const
{
	// Return default render scale (1.0, 1.0)
	x = 1.0;
	y = 1.0;
}
OFX::Host::Param::Instance *
OlivePluginInstance::newParam(const std::string &name,
						OFX::Host::Param::Descriptor &desc)
{
	if (!node_) {
		return nullptr;
	}
	const std::string &type = desc.getType();

	if (type == kOfxParamTypeInteger) {
		return new IntegerInstance(node_, desc);
	} else if (type == kOfxParamTypeDouble) {
		return new DoubleInstance(node_, name, desc);
	} else if (type == kOfxParamTypeBoolean) {
		return new BooleanInstance(node_, name, desc);
	} else if (type == kOfxParamTypeChoice) {
		return new ChoiceInstance(node_, name, desc);
	} else if (type == kOfxParamTypeString) {
		return new StringInstance(node_, name, desc);
	} else if (type == kOfxParamTypeRGBA) {
		return new RGBAInstance(node_, name, desc);
	} else if (type == kOfxParamTypeRGB) {
		return new RGBInstance(node_, name, desc);
	} else if (type == kOfxParamTypeDouble2D) {
		return new Double2DInstance(node_, name, desc);
	} else if (type == kOfxParamTypeInteger2D) {
		return new Integer2DInstance(node_, name, desc);
	} else if (type == kOfxParamTypeDouble3D) {
		return new Double3DInstance(node_, name, desc);
	} else if (type == kOfxParamTypeInteger3D) {
		return new Integer3DInstance(node_, name, desc);
	} else if (type == kOfxParamTypeCustom ||
			   type == kOfxParamTypeBytes) {
		return new CustomInstance(node_, name, desc);
	} else if (type == kOfxParamTypeGroup) {
		return new GroupInstance(desc);
	} else if (type == kOfxParamTypePage) {
		return new PageInstance(desc);
	} else if (type == kOfxParamTypePushButton) {
		return new PushbuttonInstance(node_, name, desc);
	}

	return nullptr; // 未实现的类型
}
OfxStatus OlivePluginInstance::editBegin(const std::string &name)
{
}

OFX::Host::ImageEffect::ClipInstance *OlivePluginInstance::newClipInstance(
	OFX::Host::ImageEffect::Instance *plugin,
	OFX::Host::ImageEffect::ClipDescriptor *descriptor,
	int index)
{
	// Create a new clip instance
	OFX::Host::ImageEffect::ClipInstance* clipInstance = new OliveClipInstance(plugin, *descriptor, params_);
	return clipInstance;
}

}
}
