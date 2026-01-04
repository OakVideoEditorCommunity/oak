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

//
// Created by mikesolar on 25-10-19.
//

#ifndef PLUGINRENDERER_H
#define PLUGINRENDERER_H
#include <QOpenGLExtraFunctions>
#include <QOpenGLBuffer>
#include <QOpenGLFunctions>
#include <QOpenGLShader>
#include <QOpenGLVertexArrayObject>
#include <QThread>
#include <QOffscreenSurface>

#include "render/renderer.h"
#include "render/job/pluginjob.h"
#include "render/opengl/openglrenderer.h"
namespace olive
{
namespace plugin{
class PluginRenderer : public olive::OpenGLRenderer{
	Q_OBJECT
public:
	PluginRenderer(QObject *parent=nullptr):OpenGLRenderer(parent){};
	virtual ~PluginRenderer() override{};
protected:
	virtual void RenderPlugin(TexturePtr src, olive::plugin::PluginJob& job,
					  olive::Texture *destination,
					  olive::VideoParams destination_params,
					  bool clear_destination, bool interactive);

};
}
}



#endif //PLUGINRENDERER_H
