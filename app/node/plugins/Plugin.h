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

#ifndef PLUGIN_NODES_H
#define PLUGIN_NODES_H
#include "node/node.h"
#include "pluginSupport/Plugin.h"
#include "pluginSupport/OlivePlugin.cpp"
namespace olive
{
namespace plugin
{

class PluginNode : public olive::Node{
public:
	PluginNode():Node()
	{
		memset(&methods, 0, sizeof(olive_node_methods));
	};
	explicit PluginNode(struct olive_node_methods methods):Node()
	{
		this->methods=methods;
	}PLUGIN_H
	void setMethods(struct olive_node_methods methods)
	{
		this->methods=methods;
	}
	QString Name() const override;
	QString id() const override;
	QVector<CategoryID> Category() const override;
	QString Description() const override;
	void Value(const NodeValueRow &value,
		const NodeGlobals &globals, NodeValueTable *table) const override;
private:
	struct olive_node_methods methods;

};

}
}


#endif //PLUGIN_H
