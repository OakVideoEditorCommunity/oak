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
#include "render/videoparams.h"

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
	const std::string &getDefaultOutputFielding() const{
		return kOfxImageFieldNone;
	};

	void setVideoParam(VideoParams params)
	{
		this->params_=params;
	}
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

	void getProjectSize(double& xSize, double& ySize) const override;
	void getProjectOffset(double& xOffset, double& yOffset) const override;
	void getProjectExtent(double& xSize, double& ySize) const override;
	// The pixel aspect ratio of the current project 
	double getProjectPixelAspectRatio() const override;

	// The duration of the effect 
	// This contains the duration of the plug-in effect, in frames. 
	double getEffectDuration() const override;

	// For an instance, this is the frame rate of the project the effect is in. 
	double getFrameRate() const override;

	/// This is called whenever a param is changed by the plugin so that
	/// the recursive instanceChangedAction will be fed the correct frame 
	double getFrameRecursive() const override;

	/// This is called whenever a param is changed by the plugin so that
	/// the recursive instanceChangedAction will be fed the correct
	/// renderScale
	void getRenderScaleRecursive(double &x, double &y) const override;

	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	// overridden for Param::SetInstance

	/// make a parameter instance
	///
	/// Client host code needs to implement this
	virtual OFX::Host::Param::Instance* newParam(const std::string& name, OFX::Host::Param::Descriptor& Descriptor);

	/// Triggered when the plug-in calls OfxParameterSuiteV1::paramEditBegin
	///
	/// Client host code needs to implement this
	virtual OfxStatus editBegin(const std::string& name);

	/// Triggered when the plug-in calls OfxParameterSuiteV1::paramEditEnd
	///
	/// Client host code needs to implement this
	virtual OfxStatus editEnd();

	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	// overridden for Progress::ProgressI

	/// Start doing progress.
	virtual void progressStart(const std::string &message, const std::string &messageid);

	/// finish yer progress
	virtual void progressEnd();

	/// set the progress to some level of completion, returns
	/// false if you should abandon processing, true to continue
	virtual bool progressUpdate(double t);

	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	////////////////////////////////////////////////////////////////////////////////
	// overridden for TimeLine::TimeLineI

	/// get the current time on the timeline. This is not necessarily the same
	/// time as being passed to an action (eg render)
	virtual double timeLineGetTime();

	/// set the timeline to a specific time
	virtual void timeLineGotoTime(double t);

	/// get the first and last times available on the effect's timeline
	virtual void timeLineGetBounds(double &t1, double &t2);
private:
	QList<PersistentErrors> persistentErrors_;
	VideoParams params_;
};
}
}