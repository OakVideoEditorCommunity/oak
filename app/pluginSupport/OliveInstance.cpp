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
#include "OliveInstance.h"

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
namespace olive
{
namespace plugin
{
OfxStatus OliveInstance::vmessage(const char *type, const char *id,
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
OfxStatus OliveInstance::setPersistentMessage(const char *type, const char *id,
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
	return kOfxStatOK;
}
OfxStatus OliveInstance::clearPersistentMessage()
{
	persistentErrors_.clear();
	// TODO: tell the shell to remove message.
	return kOfxStatOK;
}
void OliveInstance::getProjectSize(double &xSize, double &ySize) const
{
	xSize =params_.width();
	ySize =params_.height();
}
void OliveInstance::getProjectOffset(double &xOffset, double &yOffset) const
{
	xOffset =params_.x();
	yOffset =params_.y();
}
void OliveInstance::getProjectExtent(double &xSize, double &ySize) const
{
	xSize =params_.width();
	ySize =params_.height();
	// TODO: Ensure this project does not support this.
}
double OliveInstance::getProjectPixelAspectRatio() const
{
	return Current::getInstance()
		.currentVideoParams()
		.pixel_aspect_ratio()
		.toDouble();
}
double OliveInstance::getFrameRate() const
{
	return params_.frame_rate().toDouble();
}

double OliveInstance::getEffectDuration() const
{
	// Return a default duration value
	return 100.0;
}

double OliveInstance::getFrameRecursive() const
{
	// Return current frame (this would typically be set by the host during rendering)
	return 0.0;
}

void OliveInstance::getRenderScaleRecursive(double &x, double &y) const
{
	// Return default render scale (1.0, 1.0)
	x = 1.0;
	y = 1.0;
}
OFX::Host::Param::Instance *
OliveInstance::newParam(const std::string &name,
						OFX::Host::Param::Descriptor &Descriptor)
{

}

OFX::Host::ImageEffect::ClipInstance *OliveInstance::newClipInstance(
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
