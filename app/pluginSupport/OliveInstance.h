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

#include "ofxCore.h"
#include "ofxImageEffect.h"
#include <QString>
#include "ofxhImageEffect.h"
#include <map>
#include <qcontainerfwd.h>
#include <qlist.h>
namespace olive {
namespace plugin{
enum class ErrorType{
	Error,
	Warning,
	Message
};
struct PersistentErrors{
	ErrorType type;
	QString message;
};
class OliveInstance : public OFX::Host::ImageEffect::Instance {
public:
	OliveInstance(
				  OFX::Host::ImageEffect::ImageEffectPlugin* plugin,
				  OFX::Host::ImageEffect::Descriptor& desc,
				  const std::string& context,
				  bool interactive)
		: OFX::Host::ImageEffect::Instance(plugin, desc, context, interactive)
	{
	}
	~OliveInstance() override = default;
	virtual const std::string &getDefaultOutputFielding() const{
		return kOfxImageFieldNone;
	};

	OFX::Host::ImageEffect::ClipInstance *newClipInstance(
		OFX::Host::ImageEffect::Instance *plugin,
		OFX::Host::ImageEffect::ClipDescriptor *descriptor,
		int index) override;

	OfxStatus vmessage(const char* type,
        const char* id,
        const char* format,	
        va_list args) override;  

	OfxStatus setPersistentMessage(const char* type,
        const char* id,
        const char* format,
        va_list args)  override;
		
    OfxStatus clearPersistentMessage() override;  
private:
	QList<PersistentErrors> persistentErrors_;

};
}
}