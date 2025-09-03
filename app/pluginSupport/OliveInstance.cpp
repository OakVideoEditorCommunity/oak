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
#include "OliveInstance.h"
#include "ofxCore.h"
#include "ofxMessage.h"
#include <cstdio>
#include <QMessageBox>
#include <qmessagebox.h>
#include <string.h>
#include <QString>
namespace olive{
namespace plugin {
OfxStatus OliveInstance::vmessage(const char* type,
	const char* id,
	const char* format,	
	va_list args){

	char *buffer=new char[1024];

	memset(buffer, 0, 1024*sizeof(char));
	vsprintf(buffer, format, args);

	QString message(buffer);

	delete [] buffer;

	if(strncmp(type, kOfxMessageQuestion, strlen(kOfxMessageQuestion))){

		auto ret= QMessageBox::information(nullptr, 
			"", message, 
			QMessageBox::Ok, QMessageBox::Cancel);

		if(ret==QMessageBox::Ok){
			return kOfxStatReplyYes;
		} 
		else {
			return kOfxStatReplyNo;
		}	

	}
	else{
		QMessageBox::information(nullptr, 
			"", message);
		return kOfxStatOK;
	}
}

}
}
