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
#include <OpenImageIO/simd.h>
#include <oneapi/tbb/detail/_template_helpers.h>

OfxStatus propSetPointer(OfxPropertySetHandle properties,
	const char *property, int index, void *value)
{
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetString(OfxPropertySetHandle properties,
	const char *property, int index, const char *value)
{
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setProp(property, index, const_cast<char *>(value));
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetDouble(OfxPropertySetHandle properties,
	const char *property, int index, double value)
{

	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetInt(OfxPropertySetHandle properties, const char *property,
					 int index, int value)
{

	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetPointerN(OfxPropertySetHandle properties,
	const char *property, int count, void * const*value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setPropN(property, count, const_cast<void**>(value));
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetStringN(OfxPropertySetHandle properties,
	const char *property, int index, int count,const char *const *value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setPropN(property, count, const_cast<char**>(value));
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetDoubleN(OfxPropertySetHandle properties,
	const char *property,int count, const double* value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setPropN(property, count, const_cast<double *>(value));
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetIntN(OfxPropertySetHandle properties, const char *property,
					 int count, const int *value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->setPropN(property, count, const_cast<int *>(value));
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetPointer(OfxPropertySetHandle properties,
	const char *property, int index, void **value)
{
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetString(OfxPropertySetHandle properties,
	const char *property, int index, char **value)
{
	std::any val;
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetDouble(OfxPropertySetHandle properties,
	const char *property, int index, double *value)
{
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetInt(OfxPropertySetHandle properties, const char *property,
					 int index, int *value)
{
	if (properties->plugin) {
		if (index<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getProp(property, index, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetPointerN(OfxPropertySetHandle properties,
	const char *property, int count, void ***value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getPropN(property, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propGetStringN(OfxPropertySetHandle properties,
	const char *property, int count, char ***value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getPropN(property, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetDoubleN(OfxPropertySetHandle properties,
	const char *property, int index, int count, double** value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getPropN(property, value);
	}
	return kOfxStatErrBadHandle;
}

OfxStatus propSetIntN(OfxPropertySetHandle properties, const char *property,
					 int index, int count, int **value)
{
	if (properties->plugin) {
		if (count<0) {
			return kOfxStatErrBadIndex;
		}
		return properties->plugin->getPropN(property, value);
	}
	return kOfxStatErrBadHandle;
}
