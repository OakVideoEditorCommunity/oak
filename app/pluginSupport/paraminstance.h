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

// Copyright OpenFX and contributors to the OpenFX project.
#ifndef PARAM_INSTANCE_H
#define PARAM_INSTANCE_H

#include "OlivePluginInstance.h"
#include "ofxhParam.h"
namespace olive
{
namespace plugin
{
class PushbuttonInstance : public OFX::Host::Param::PushbuttonInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor *_descriptor;
public:
	PushbuttonInstance(OlivePluginInstance *effect, const std::string &name,
					   OFX::Host::Param::Descriptor &descriptor)
		: OFX::Host::Param::PushbuttonInstance( descriptor)
	{
		_descriptor = &descriptor;
	};
};

class IntegerInstance : public OFX::Host::Param::IntegerInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	IntegerInstance(OlivePluginInstance* effect, const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(int&);
	OfxStatus get(OfxTime time, int&);
	OfxStatus set(int);
	OfxStatus set(OfxTime time, int);
};

class DoubleInstance : public OFX::Host::Param::DoubleInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	DoubleInstance(OlivePluginInstance* effect, const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(double&);
	OfxStatus get(OfxTime time, double&);
	OfxStatus set(double);
	OfxStatus set(OfxTime time, double);
	OfxStatus derive(OfxTime time, double&);
	OfxStatus integrate(OfxTime time1, OfxTime time2, double&);
};

class BooleanInstance : public OFX::Host::Param::BooleanInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	BooleanInstance(OlivePluginInstance* effect, const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(bool&);
	OfxStatus get(OfxTime time, bool&);
	OfxStatus set(bool);
	OfxStatus set(OfxTime time, bool);
};

class ChoiceInstance : public OFX::Host::Param::ChoiceInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	ChoiceInstance(OlivePluginInstance* effect,  const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(int&);
	OfxStatus get(OfxTime time, int&);
	OfxStatus set(int);
	OfxStatus set(OfxTime time, int);
};

class RGBAInstance : public OFX::Host::Param::RGBAInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	RGBAInstance(OlivePluginInstance* effect, const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(double&,double&,double&,double&);
	OfxStatus get(OfxTime time, double&,double&,double&,double&);
	OfxStatus set(double,double,double,double);
	OfxStatus set(OfxTime time, double,double,double,double);
};


class RGBInstance : public OFX::Host::Param::RGBInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	RGBInstance(OlivePluginInstance* effect,  const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(double&,double&,double&);
	OfxStatus get(OfxTime time, double&,double&,double&);
	OfxStatus set(double,double,double);
	OfxStatus set(OfxTime time, double,double,double);
};

class Double2DInstance : public OFX::Host::Param::Double2DInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	Double2DInstance(OlivePluginInstance* effect, const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(double&,double&);
	OfxStatus get(OfxTime time,double&,double&);
	OfxStatus set(double,double);
	OfxStatus set(OfxTime time,double,double);
};

class Integer2DInstance : public OFX::Host::Param::Integer2DInstance {
protected:
	OlivePluginInstance*   _effect;
	OFX::Host::Param::Descriptor& _descriptor;
public:
	Integer2DInstance(OlivePluginInstance* effect,  const std::string& name, OFX::Host::Param::Descriptor& descriptor);
	OfxStatus get(int&,int&);
	OfxStatus get(OfxTime time,int&,int&);
	OfxStatus set(int,int);
	OfxStatus set(OfxTime time,int,int);
};
}
}



#endif // HOST_DEMO_PARAM_INSTANCE_H


#endif //PARAMINSTANCE_H
