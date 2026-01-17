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

#include <algorithm>
#include <cstring>
#include <olive/core/core.h>
#include <QByteArray>
#include <QHash>
#include <QVariant>
#include <QVector2D>
#include <QVector3D>

namespace {
QHash<QString, QHash<QString, QVariant>> g_plugin_param_defaults;

QVariant DefaultValueForParam(const OFX::Host::Param::Base *param)
{
	if (!param) {
		return QVariant();
	}
	const std::string &ofxType = param->getType();
	const auto &props = param->getProperties();

	if (ofxType == kOfxParamTypeInteger ||
		ofxType == kOfxParamTypeChoice) {
		return props.getIntProperty(kOfxParamPropDefault);
	}
	if (ofxType == kOfxParamTypeBoolean) {
		return props.getIntProperty(kOfxParamPropDefault) != 0;
	}
	if (ofxType == kOfxParamTypeDouble) {
		return props.getDoubleProperty(kOfxParamPropDefault);
	}
	if (ofxType == kOfxParamTypeString ||
		ofxType == kOfxParamTypeStrChoice ||
		ofxType == kOfxParamTypeCustom) {
		return QString::fromStdString(
			props.getStringProperty(kOfxParamPropDefault));
	}
	if (ofxType == kOfxParamTypeRGB ||
		ofxType == kOfxParamTypeRGBA) {
		const int count = (ofxType == kOfxParamTypeRGBA) ? 4 : 3;
		double values[4] = {0.0, 0.0, 0.0, 1.0};
		props.getDoublePropertyN(kOfxParamPropDefault, values, count);
		const double alpha = (count == 4) ? values[3] : 1.0;
		return QVariant::fromValue(
			olive::core::Color(values[0], values[1], values[2], alpha));
	}
	if (ofxType == kOfxParamTypeDouble2D ||
		ofxType == kOfxParamTypeDouble3D ||
		ofxType == kOfxParamTypeInteger2D ||
		ofxType == kOfxParamTypeInteger3D) {
		const bool is_double =
			(ofxType == kOfxParamTypeDouble2D ||
			 ofxType == kOfxParamTypeDouble3D);
		const int count = (ofxType == kOfxParamTypeDouble2D ||
						   ofxType == kOfxParamTypeInteger2D)
			? 2
			: 3;
		if (is_double) {
			double values[3] = {0.0, 0.0, 0.0};
			props.getDoublePropertyN(kOfxParamPropDefault, values, count);
			if (count == 2) {
				return QVector2D(values[0], values[1]);
			}
			return QVector3D(values[0], values[1], values[2]);
		}
		int values[3] = {0, 0, 0};
		props.getIntPropertyN(kOfxParamPropDefault, values, count);
		if (count == 2) {
			return QVector2D(values[0], values[1]);
		}
		return QVector3D(values[0], values[1], values[2]);
	}
	if (ofxType == kOfxParamTypeBytes) {
		return QByteArray();
	}

	return QVariant();
}

QHash<QString, QVariant>
BuildDefaultValues(const std::map<std::string, OFX::Host::Param::Instance *> &params)
{
	QHash<QString, QVariant> defaults;
	for (const auto &param : params) {
		const std::string &ofxType = param.second->getType();
		if (ofxType == kOfxParamTypeGroup ||
			ofxType == kOfxParamTypePage ||
			ofxType == kOfxParamTypePushButton) {
			continue;
		}
		const auto &props = param.second->getProperties();
		if (props.getIntProperty(kOfxParamPropSecret) != 0) {
			continue;
		}
		const QString input_id =
			QString::fromStdString(param.second->getName());
		if (input_id.isEmpty()) {
			continue;
		}
		QVariant default_value = DefaultValueForParam(param.second);
		if (!default_value.isValid()) {
			continue;
		}
		defaults.insert(input_id, default_value);
	}
	return defaults;
}
}
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
	const QString plugin_id = QString::fromStdString(
		plugin_instance_->getPlugin()->getIdentifier());
	auto defaults_iter = g_plugin_param_defaults.find(plugin_id);
	if (defaults_iter == g_plugin_param_defaults.end()) {
		g_plugin_param_defaults.insert(plugin_id, BuildDefaultValues(params));
		defaults_iter = g_plugin_param_defaults.find(plugin_id);
	}
	const QHash<QString, QVariant> &defaults = defaults_iter.value();
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
		QVariant default_value = defaults.value(input_id, QVariant());
		if (default_value.isValid()) {
			AddInput(input_id, type, default_value);
			if (type != NodeValue::kPushButton) {
				SetStandardValue(input_id, default_value);
			}
		} else {
			AddInput(input_id, type);
		}
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
		if (type == NodeValue::kCombo || type == NodeValue::kStrCombo) {
			QStringList option_labels;
			QStringList option_values;
			const int label_count =
				props.getDimension(kOfxParamPropChoiceOption);
			const int value_count =
				props.getDimension(kOfxParamPropChoiceEnum);

			for (int i = 0; i < label_count; ++i) {
				const std::string &label =
					props.getStringProperty(kOfxParamPropChoiceOption, i);
				option_labels.append(QString::fromStdString(label));
			}

			for (int i = 0; i < value_count; ++i) {
				const std::string &value =
					props.getStringProperty(kOfxParamPropChoiceEnum, i);
				option_values.append(QString::fromStdString(value));
			}

			if (option_labels.isEmpty() && !option_values.isEmpty()) {
				option_labels = option_values;
			}
			if (option_values.isEmpty() && !option_labels.isEmpty()) {
				option_values = option_labels;
			}

			const int order_count =
				props.getDimension(kOfxParamPropChoiceOrder);
			if (order_count == option_labels.size() &&
				option_labels.size() == option_values.size()) {
				QVector<int> indices(option_labels.size());
				for (int i = 0; i < indices.size(); ++i) {
					indices[i] = i;
				}

				std::stable_sort(indices.begin(), indices.end(),
								 [&](int a, int b) {
									 return props.getIntProperty(
												kOfxParamPropChoiceOrder,
												a) <
											props.getIntProperty(
												kOfxParamPropChoiceOrder,
												b);
								 });

				QStringList ordered_labels;
				QStringList ordered_values;
				for (int index : indices) {
					ordered_labels.append(option_labels.at(index));
					ordered_values.append(option_values.at(index));
				}
				option_labels = ordered_labels;
				option_values = ordered_values;
			}

			if (!option_labels.isEmpty()) {
				SetComboBoxStrings(input_id, option_labels);
				if (type == NodeValue::kStrCombo) {
					SetInputProperty(input_id,
									 QStringLiteral("combo_value_str"),
									 option_values);
				}
			}
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
	

	const QString source_id =
		QString::fromUtf8(kOfxImageEffectSimpleSourceClipName);
	if (HasInputWithID(source_id)) {
		SetEffectInput(source_id);
	} else if (HasInputWithID(kTextureInput)) {
		SetEffectInput(kTextureInput);
	}
	else {
		if (has_texture_input) {
			AddInput(kTextureInput, NodeValue::kTexture);
			SetInputName(kTextureInput, tr("Texture"));
			SetEffectInput(kTextureInput);
		}
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
	Q_UNUSED(values)
	Q_UNUSED(index)

	if (!input.is_allocated() || input.channel_count() == 0 ||
		input.sample_count() == 0) {
		if (output.is_allocated()) {
			output.silence();
		}
		return;
	}

	if (!output.is_allocated() ||
		output.channel_count() != input.channel_count() ||
		output.sample_count() != input.sample_count()) {
		output.set_audio_params(input.audio_params());
		output.set_sample_count(input.sample_count());
		output.allocate();
	}

	if (!output.is_allocated()) {
		return;
	}

	for (int channel = 0; channel < input.channel_count(); ++channel) {
		output.fast_set(input, channel);
	}
}

void olive::plugin::PluginNode::GenerateFrame(FramePtr frame,
											  const GenerateJob &job) const
{
	Q_UNUSED(job)

	if (!frame) {
		return;
	}

	if (!frame->is_allocated()) {
		frame->allocate();
	}

	if (!frame->is_allocated()) {
		return;
	}

	std::memset(frame->data(), 0, static_cast<size_t>(frame->allocated_size()));
}
void olive::plugin::PluginNode::Value(const NodeValueRow &value,
									  const NodeGlobals &globals,
									  NodeValueTable *table) const
{
	for (auto it = value.cbegin(); it != value.cend(); ++it) {
		const NodeValue &input_value = it.value();
		if (input_value.type() == NodeValue::kTexture ||
			input_value.type() == NodeValue::kNone) {
			continue;
		}
		NodeValue tagged = input_value;
		tagged.set_tag(it.key());
		table->Push(tagged);
	}

	TexturePtr tex = nullptr;
	const QString source_key =
		QString::fromUtf8(kOfxImageEffectSimpleSourceClipName);
	if (value.contains(source_key)) {
		tex = value.value(source_key).toTexture();
	}
	if (!tex) {
		tex = value.value(kTextureInput).toTexture();
	}
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
