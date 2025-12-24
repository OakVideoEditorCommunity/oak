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

#include "Plugin.h"

#include "render/rendermanager.h"
#include "render/job/pluginjob.h"
olive::plugin::PluginNode::PluginNode(
	OFX::Host::ImageEffect::Instance *plugin)
{
	plugin_instance_=plugin;

	auto params=plugin_instance_->getParams();
	for (auto param: params) {

		NodeValue::Type type = NodeValue::kNone;

		const std::string &ofxType = param.second->getType();
		if (ofxType == kOfxParamTypeInteger) {
			type = NodeValue::kInt;
		} else if (ofxType == kOfxParamTypeDouble) {
			type = NodeValue::kFloat;
		} else if (ofxType == kOfxParamTypeBoolean) {
			type = NodeValue::kBoolean;
		} else if (ofxType == kOfxParamTypeString) {
			type = NodeValue::kText;
		} else if (ofxType == kOfxParamTypeRGB ||
				   ofxType == kOfxParamTypeRGBA) {
			type = NodeValue::kColor;
		} else if (ofxType == kOfxParamTypeChoice) {
			type = NodeValue::kCombo;
		} else if (ofxType == kOfxParamTypeDouble2D ||
			       ofxType == kOfxParamTypeInteger2D){
			type = NodeValue::kVec2;}
		else if (ofxType == kOfxParamTypeDouble3D ||
		         ofxType == kOfxParamTypeInteger3D){
			type = NodeValue::kVec3;
		} else if (ofxType == kOfxParamTypeStrChoice){
			type = NodeValue::kStrCombo;
		}else if (ofxType == kOfxParamTypeBytes
			|| ofxType == kOfxParamTypeCustom) {
			type = NodeValue::kBinary;
		} else if (ofxType == kOfxParamTypePushButton) {
			type = NodeValue::kPushButton;
		} else if (ofxType == kOfxParamTypeGroup) {
			// TODO
		} else if (ofxType == kOfxParamTypePage) {
			// TODO
		}else {
			type = NodeValue::kNone;
		}



		AddInput(param.second->getName().data(), type);
	}

}
QString olive::plugin::PluginNode::Name() const
{
	const auto *plugin = plugin_instance_->getPlugin();
	return plugin->getDescriptor()
		.getProps()
		.getStringProperty(kOfxPropLabel)
		.data();

}

QString olive::plugin::PluginNode::Description() const
{
	const auto *plugin = plugin_instance_->getPlugin();
	return plugin->getDescriptor()
		.getProps()
		.getStringProperty(kOfxPropPluginDescription)
		.data();

}
void olive::plugin::PluginNode::Value(const NodeValueRow &value,
									  const NodeGlobals &globals,
									  NodeValueTable *table) const
{
	TexturePtr tex = value[kTextureInput].toTexture();
	if (tex && plugin_instance_) {
		PluginJob job(plugin_instance_, value);
		table->Push(NodeValue::kTexture, tex->toJob(job), this);
	}
}
void olive::plugin::PluginNode::pushButtonClicked(QString name)
{
}

QString olive::plugin::PluginNode::id() const
{
	const auto *plugin = plugin_instance_->getPlugin();
	return plugin->getIdentifier().data();
}

olive::Node *olive::plugin::PluginNode::copy() const
{
	auto *node = new PluginNode(new OlivePluginInstance(*plugin_instance_));
	if (!plugin_instance_) {
		return node;
	}

	const auto &contexts = plugin_instance_->getPlugin()->getContexts();
	std::string context = kOfxImageEffectContextFilter;
	if (!contexts.empty()) {
		if (contexts.find(kOfxImageEffectContextFilter) == contexts.end()) {
			context = *contexts.begin();
		}
	}

	node->setPluginInstance(
		plugin_instance_->getPlugin()->createInstance(context, node));
	return node;
}
