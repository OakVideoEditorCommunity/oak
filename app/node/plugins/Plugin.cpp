/*
 * Oak Video Editor - Non-Linear Video Editor
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
#include "pluginSupport/OlivePluginInstance.h"
static QString ClipLabelForName(const std::string &name,
								const OFX::Host::ImageEffect::ClipDescriptor *desc)
{
	if (name == kOfxImageEffectSimpleSourceClipName) {
		return olive::plugin::PluginNode::tr("Source");
	}
	if (name == kOfxImageEffectTransitionSourceFromClipName) {
		return olive::plugin::PluginNode::tr("From");
	}
	if (name == kOfxImageEffectTransitionSourceToClipName) {
		return olive::plugin::PluginNode::tr("To");
	}

	if (desc) {
		const std::string &label =
			desc->getProps().getStringProperty(kOfxPropLabel);
		if (!label.empty()) {
			return QString::fromStdString(label);
		}
	}

	return QString::fromStdString(name);
}

olive::plugin::PluginNode::PluginNode(
	OFX::Host::ImageEffect::Instance *plugin)
{
	plugin_instance_=plugin;
	bool has_texture_input = false;
	QHash<QString, QString> group_labels;
	QHash<QString, QString> page_labels;
	QHash<QString, QString> page_for_param;

	auto params=plugin_instance_->getParams();
	for (auto param: params) {
		const std::string &ofxType = param.second->getType();
		if (ofxType == kOfxParamTypeGroup) {
			const QString name = QString::fromStdString(param.first);
			const QString label =
				QString::fromStdString(param.second->getLabel());
			group_labels.insert(name, label.isEmpty() ? name : label);
		} else if (ofxType == kOfxParamTypePage) {
			const QString name = QString::fromStdString(param.first);
			const QString label =
				QString::fromStdString(param.second->getLabel());
			page_labels.insert(name, label.isEmpty() ? name : label);

			const auto &props = param.second->getProperties();
			int count = props.getDimension(kOfxParamPropPageChild);
			for (int i = 0; i < count; ++i) {
				const std::string &child =
					props.getStringProperty(kOfxParamPropPageChild, i);
				if (child == kOfxParamPageSkipRow ||
					child == kOfxParamPageSkipColumn) {
					continue;
				}
				page_for_param.insert(QString::fromStdString(child),
									  page_labels.value(name));
			}
		}
	}

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
		} else if (ofxType == kOfxParamTypeGroup ||
				   ofxType == kOfxParamTypePage) {
			continue;
		}else {
			type = NodeValue::kNone;
		}

		const QString input_id = QString::fromStdString(param.second->getName());
		if (input_id.isEmpty()) {
			continue;
		}
		const auto &props = param.second->getProperties();
		if (props.getIntProperty(kOfxParamPropSecret) != 0) {
			continue;
		}
		if (type == NodeValue::kNone) {
			continue;
		}
		AddInput(input_id, type);
		const QString label =
			QString::fromStdString(param.second->getLabel());
		if (!label.isEmpty()) {
			SetInputName(input_id, label);
		} else {
			SetInputName(input_id, input_id);
		}
		const QString parent =
			QString::fromStdString(param.second->getParentName());
		if (!parent.isEmpty()) {
			SetInputProperty(input_id, QStringLiteral("ui_group"),
							 group_labels.value(parent, parent));
		}
		if (page_for_param.contains(input_id)) {
			SetInputProperty(input_id, QStringLiteral("ui_page"),
							 page_for_param.value(input_id));
		}
	}

	const auto &clips = plugin_instance_->getDescriptor().getClips();
	for (const auto &entry : clips) {
		if (entry.first == kOfxImageEffectOutputClipName) {
			continue;
		}
		QString input_id = QString::fromStdString(entry.first);
		AddInput(input_id, NodeValue::kTexture);
		SetInputName(input_id, ClipLabelForName(entry.first, entry.second));
		has_texture_input = true;
	}
	if (!has_texture_input) {
		AddInput(kTextureInput, NodeValue::kTexture);
		SetInputName(kTextureInput, tr("Texture"));
	}
}

olive::plugin::PluginNode::~PluginNode() = default;
QString olive::plugin::PluginNode::Name() const
{
	const auto *plugin = plugin_instance_->getPlugin();
	return plugin->getDescriptor()
		.getProps()
		.getStringProperty(kOfxPropLabel)
		.data();

}

QVector<olive::Node::CategoryID> olive::plugin::PluginNode::Category() const
{
	return { olive::Node::kCategoryUnknown };
}

QString olive::plugin::PluginNode::Description() const
{
	const auto *plugin = plugin_instance_->getPlugin();
	return plugin->getDescriptor()
		.getProps()
		.getStringProperty(kOfxPropPluginDescription)
		.data();

}
void olive::plugin::PluginNode::ProcessSamples(const NodeValueRow &values,
											   const SampleBuffer &input,
											   SampleBuffer &output,
											   int index) const
{
	(void)values;
	(void)input;
	(void)output;
	(void)index;
}

void olive::plugin::PluginNode::GenerateFrame(FramePtr frame,
											  const GenerateJob &job) const
{
	(void)frame;
	(void)job;
}
void olive::plugin::PluginNode::Value(const NodeValueRow &value,
									  const NodeGlobals &globals,
									  NodeValueTable *table) const
{
	TexturePtr tex = value.value(kTextureInput).toTexture();
	if (!tex) {
		for (auto it = value.cbegin(); it != value.cend(); ++it) {
			if (it.value().type() == NodeValue::kTexture) {
				tex = it.value().toTexture();
				if (tex) {
					break;
				}
			}
		}
	}
	if (tex && plugin_instance_) {
		PluginJob job(plugin_instance_, this, value, globals.time().in());
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
		if (!plugin_instance_) {
			return nullptr;
		}

		const auto &contexts = plugin_instance_->getPlugin()->getContexts();
		std::string context = kOfxImageEffectContextFilter;
		if (!contexts.empty() &&
			contexts.find(kOfxImageEffectContextFilter) == contexts.end()) {
			context = *contexts.begin();
		}

		auto *instance =
			plugin_instance_->getPlugin()->createInstance(context, nullptr);
		if (!instance) {
			return nullptr;
		}

		auto *node = new PluginNode(instance);
		if (auto *olive_instance =
				dynamic_cast<OlivePluginInstance *>(instance)) {
			olive_instance->setNode(
				std::shared_ptr<PluginNode>(node, [](PluginNode *) {}));
		}
		return node;
	}
