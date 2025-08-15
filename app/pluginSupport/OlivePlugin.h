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

#ifndef OLIVE_PLUGIN_H
#define OLIVE_PLUGIN_H
#include "OlivePlugin.h"
#include "ofxCore.h"
#include <QVariant>
#include <cstdint>
#include <QString>
#include <QMap>
#include <any>
#include <list>
#include <OpenImageIO/detail/fmt/chrono.h>
namespace olive
{
namespace plugin
{
class Value;
enum class Type {
	FLOAT, STRING, INT, POINTER, ARRAY, NONE
};
struct plugin_mem_ptr {
	void *mem;
	int32_t count=0;
};
class OlivePlugin {
public:
	OlivePlugin()=default;
	~OlivePlugin();
	OfxStatus getProp(QString key, int index, char **str);
	OfxStatus getPropN(QString key, char *** strs);
	OfxStatus getProp(QString key, int index, void **p);
	OfxStatus getPropN(QString key, void ***ps);
	OfxStatus getProp(QString key, int index, int *p);
	OfxStatus getPropN(QString key, int **ps);
	OfxStatus getProp(QString key, int index, double *p);
	OfxStatus getPropN(QString key, double **ps);
	OfxStatus setProp(QString key, int index, char *str);
	OfxStatus setPropN(QString key,int count, char ** strs);
	OfxStatus setProp(QString key, int index, void *p);
	OfxStatus setPropN(QString key,int count, void **ps);
	OfxStatus setProp(QString key, int index, int p);
	OfxStatus setPropN(QString key,int count, int *ps);
	OfxStatus setProp(QString key, int index, double p);
	OfxStatus setPropN(QString key, int count, double *ps);
	void addMem(void *mem)
	{
		mems.emplace_back(mem);
	}
private:
	uint64_t id;
	QString name;
	QString version;
	QMap<QString, QVector<char *>> stringProps;
	QMap<QString, QVector<int>> intProps;
	QMap<QString, QVector<double>> doubleProps;
	QMap<QString, QVector<void *>> pointerProps;
	std::list<void *> mems;
};
}
}
struct OfxPropertySetStruct {
	olive::plugin::OlivePlugin* plugin;
};
#endif //OLIVE_PLUGIN_H
