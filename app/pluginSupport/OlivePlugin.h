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
#include "ofxCore.h"

#include <cstdint>
#include <QString>
#include <QMap>
#include <list>
namespace olive
{
namespace plugin
{
class Value;
enum class Type {
	FLOAT, STRING, INT, POINTER, ARRAY, NONE
};
class Array {
private:
	Type type;
	int strSize=0;
	char** strings;
	QVector<void *> pointers;
	QVector<int> ints;
	QVector<double> floats;
public:
	Array()
	{
		type=Type::NONE;
	}
	Array(const int *data, size_t size)
	{
		this->type=Type::INT;
		ints.reserve(size);
		std::copy(data, data+size, ints.data());
	}
	Array(const double *data, size_t size)
	{
		this->type=Type::FLOAT;
		floats.reserve(size);
		std::copy(data, data+size, floats.data());
	}
	Array(void * const* data, size_t size)
	{
		this->type=Type::POINTER;
		pointers.reserve(size);
		std::copy(data, data+size, pointers.data());
	}
	Array(const char * const *data, size_t size)
	{
		strings=(char**)malloc(sizeof(char *)*size);
		for (int i=0;i<size;i++) {
			int len=strlen(data[i]);
			strings[i]=(char*)malloc(sizeof(char)*len);
			strncpy(strings[i], data[i], len);
		}
		strSize=size;
	}
	void setArray(int *data, size_t size)
	{
		this->type=Type::INT;
		ints.clear();
		ints.reserve(size);
		std::copy(data, data+size, ints.data());
	}
	void setArray(double *data, size_t size)
	{
		this->type=Type::FLOAT;
		floats.clear();
		floats.reserve(size);
		std::copy(data, data+size, floats.data());
	}
	void setArray(void *const*data, size_t size)
	{
		this->type=Type::POINTER;
		pointers.reserve(size);
		pointers.clear();
		std::copy(data, data+size, pointers.data());
	}
	void setArray(const char * const *data, size_t size)
	{
		for (int i=0;i<this->strSize;i++) {
			free(strings[i]);
		}
		free(strings);
		strings=nullptr;
		strings=(char**)malloc(sizeof(char *)*size);
		for (int i=0;i<size;i++) {
			int len=strlen(data[i]);
			strings[i]=(char*)malloc(sizeof(char)*len);
			strncpy(strings[i], data[i], len);
		}
		strSize=size;
	}

	int size()
	{
		switch (type) {
		case Type::INT: {
			return ints.size();
		}
		case Type::FLOAT: {
			return floats.size();
		}
		case Type::POINTER: {
			return pointers.size();
		}
		case Type::STRING: {
			return strSize;
		}
		}
	}
	int *getInts(int *size)
	{
		*size=ints.size();
		return ints.data();
	}
	double *getFLoats(int *size)
	{
		*size=floats.size();
		return floats.data();
	}
	void **getPointers(int *size)
	{
		*size=pointers.size();
		return pointers.data();
	}
	char **getStrings(int *size)
	{
		*size=strSize;
		return strings;
	}
	bool isInt()
	{
		return type==Type::INT;
	}
	bool isFloat()
	{
		return type==Type::FLOAT;
	}
	bool isString()
	{
		return type==Type::STRING;
	}
	bool isPointer()
	{
		return type==Type::POINTER;
	}
};
class Value {
private:
	int valueInt=0;
	double valueDouble=0;
	QString valueString;
	void *valuePointer;
	Type type=Type::NONE;
	Array array;
public:

	Value()=default;
	Value(int val)
	{
		valueInt=val;
		type=Type::INT;
	}
	Value(double val)
	{
		valueDouble=val;
		type=Type::FLOAT;
	}
	Value(QString val)
	{
		valueString=val;
		type=Type::STRING;
	}
	Value(void *val)
	{
		valuePointer=val;
		type=Type::POINTER;
	}
	Value(Array val)
	{
		array=val;
		type=Type::ARRAY;
	}
	void setValue(int64_t val)
	{
		valueInt=val;
		type=Type::INT;
	}
	void setValue(double val)
	{
		valueDouble=val;
		type=Type::FLOAT;
	}
	void setValue(QString val)
	{
		valueString=val;
		type=Type::STRING;
	}
	void setValue(void *val)
	{
		valuePointer=val;
		type=Type::POINTER;
	}
	void setValue(nullptr_t val)
	{
		valueInt=0;
		valueDouble=0;
		valueString=QString();
		type=Type::NONE;
	}
	void setValue(Array val)
	{
		valueInt=0;
		valueDouble=0;
		valueString=QString();
		array=val;
		type=Type::ARRAY;
	}
	int getInt() const
	{
		if (type==Type::INT)
			return valueInt;
		return 0;
	}
	double getFloat() const
	{
		if (type==Type::FLOAT)
			return valueDouble;
		return 0;
	}
	Array getArray() const
	{
		if (type==Type::ARRAY)
			return array;
		return Array();
	}
	QString getString()
	{
		return valueString;
	}
	bool isInt()
	{
		return type==Type::INT;
	}
	bool isFloat()
	{
		return type==Type::FLOAT;
	}
	bool isString()
	{
		return type==Type::STRING;
	}
	bool isPointer()
	{
		return type==Type::POINTER;
	}
	bool isArray()
	{
		return type==Type::ARRAY;
	}
	bool isNull()
	{
		return type==Type::NONE;
	}
};

struct plugin_mem_ptr {
	void *mem;
	int32_t count=0;
};
class OlivePlugin {
public:
	OlivePlugin()=default;
	~OlivePlugin();
	void setProp(QString key, int index, Value val);
	OfxStatus getProp(QString key, int index, Value &val);
	void addMem(void *mem)
	{
		mems.emplace_back(mem);
	}
private:
	uint64_t id;
	QString name;
	QString version;
	QMap<QString, QMap<int, Value>> props;
	std::list<void *> mems;
};
}
}
struct OfxPropertySetStruct {
	olive::plugin::OlivePlugin* plugin;
};
#endif //OLIVE_PLUGIN_H
