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
#include "OlivePlugin.h"

#include <utility>
olive::plugin::OlivePlugin::~OlivePlugin()
{
	for (void *p:mems) {
		free(p);
	}
}
OfxStatus olive::plugin::OlivePlugin::getProp(QString key, int index,
													char **str)
{
	*str=nullptr;
	if (stringProps.contains(key)) {
		if (stringProps[key].count()>index) {
			*str=stringProps[key][index];
			return kOfxStatOK;
		}
		return kOfxStatErrBadIndex;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getPropN(QString key,
													char ***str)
{
	*str=nullptr;
	if (stringProps.contains(key)) {
		*str=stringProps[key].data();
		return kOfxStatOK;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getProp(QString key, int index,
													void **str)
{
	*str=nullptr;
	if (pointerProps.contains(key)) {
		if (pointerProps[key].count()>index) {
			*str=pointerProps[key][index];
			return kOfxStatOK;
		}
		return kOfxStatErrBadIndex;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getPropN(QString key,
													void ***p)
{
	*p=nullptr;
	if (pointerProps.contains(key)) {
		*p=pointerProps[key].data();
		return kOfxStatOK;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getProp(QString key, int index,
													int* p)
{
	*p=0;
	if (intProps.contains(key)) {
		if (intProps[key].count()>index) {
			*p=intProps[key][index];
			return kOfxStatOK;
		}
		return kOfxStatErrBadIndex;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getPropN(QString key,
													int **p)
{
	*p=nullptr;
	if (intProps.contains(key)) {
		*p=intProps[key].data();
		return kOfxStatOK;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getProp(QString key, int index,
													double* p)
{
	*p=0;
	if (doubleProps.contains(key)) {
		if (doubleProps[key].count()>index) {
			*p=doubleProps[key][index];
			return kOfxStatOK;
		}
		return kOfxStatErrBadIndex;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::getPropN(QString key,
													double **p)
{
	*p=nullptr;
	if (doubleProps.contains(key)) {
		*p=doubleProps[key].data();
		return kOfxStatOK;
	}
	return kOfxStatErrUnknown;
}
OfxStatus olive::plugin::OlivePlugin::setProp(QString key, int index, char *str)
{
	char *p=(char*)calloc(strlen(str)+1,sizeof(char));
	strcpy(p,str);
	if (stringProps.contains(key)) {
		if (stringProps[key].count()>index) {
			stringProps[key].insert(index, p);
			return kOfxStatOK;
		}
		stringProps[key].emplace_back(p);
		return kOfxStatOK;
	}
	QVector<char *> vec;
	vec.emplace_back(str);
	stringProps[key]=vec;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setPropN(QString key, int count, char **strs)
{
	QVector<char *> vector;
	for (int i=0;i<count;i++) {
		char *p=(char*)calloc(strlen(strs[i])+1,sizeof(char));
		strcpy(p,strs[i]);
		vector.emplace_back(p);
	}
	stringProps[key]=vector;
}
OfxStatus olive::plugin::OlivePlugin::setProp(QString key, int index, void *p)
{
	if (pointerProps.contains(key)) {
		if (pointerProps[key].count()>index) {
			pointerProps[key].insert(index, p);
			return kOfxStatOK;
		}
		pointerProps[key].emplace_back(p);
		return kOfxStatOK;
	}
	QVector<void *> vec;
	vec.emplace_back(p);
	pointerProps[key]=vec;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setPropN(QString key,int count, void **ps)
{
	QVector<void *> vector;
	vector.reserve(count);
	memcpy(vector.data(), ps, sizeof(void *)*count);
	pointerProps[key]=vector;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setProp(QString key, int index, int p)
{
	if (intProps.contains(key)) {
		if (intProps[key].count()>index) {
			intProps[key].insert(index, p);
			return kOfxStatOK;
		}
		intProps[key].emplace_back(p);
		return kOfxStatOK;
	}
	QVector<int> vec;
	vec.emplace_back(p);
	intProps[key]=vec;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setPropN(QString key,int count, int *ps)
{
	QVector<int> vector;
	vector.reserve(count);
	memcpy(vector.data(), ps, sizeof(int)*count);
	intProps[key]=vector;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setProp(QString key, int index, double p)
{
	if (doubleProps.contains(key)) {
		if (doubleProps[key].count()>index) {
			doubleProps[key].insert(index, p);
			return kOfxStatOK;
		}
		doubleProps[key].emplace_back(p);
		return kOfxStatOK;
	}
	QVector<double> vec;
	vec.emplace_back(p);
	doubleProps[key]=vec;
	return kOfxStatOK;
}
OfxStatus olive::plugin::OlivePlugin::setPropN(QString key,int count, double *ps)
{
	QVector<double> vector;
	vector.reserve(count);
	memcpy(vector.data(), ps, sizeof(doubleProps)*count);
	doubleProps[key]=vector;
	return kOfxStatOK;
}

