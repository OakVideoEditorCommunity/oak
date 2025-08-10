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
#include "node/plugins/Plugin.h"

#include <ofxProperty.h>

OfxStatus propSetPointer(OfxPropertySetHandle properties,
	const char *property, int index, void *value)
{
	olive::plugin::Value val;
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, value);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetString(OfxPropertySetHandle properties,
	const char *property, int index, const char *value)
{
	olive::plugin::Value val;
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, QString(value));
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetDouble(OfxPropertySetHandle properties,
	const char *property, int index, double value)
{
	olive::plugin::Value val;
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, static_cast<double>(value));
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetInt(OfxPropertySetHandle properties, const char *property,
					 int index, int value)
{
	olive::plugin::Value val;
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, value);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetPointerN(OfxPropertySetHandle properties,
	const char *property, int index, int count, void * const*value)
{
	olive::plugin::Value val;
	olive::plugin::Array array(value,count);
	val.setValue(array);
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, val);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetStringN(OfxPropertySetHandle properties,
	const char *property, int index, int count,const char *const *value)
{
	olive::plugin::Value val;
	olive::plugin::Array array(value,count);
	val.setValue(array);
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, val);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetDoubleN(OfxPropertySetHandle properties,
	const char *property, int index, int count, const double* value)
{
	olive::plugin::Value val;
	olive::plugin::Array array(value,count);
	val.setValue(array);
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, val);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetIntN(OfxPropertySetHandle properties, const char *property,
					 int index, int count, const int *value)
{
	olive::plugin::Value val;
	olive::plugin::Array array(value, count);
	val.setValue(array);
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		properties->plugin->setProp(property, index, val);
		return kOfxStatOK;
	}
	return kOfxStatErrBadHandle;
}
