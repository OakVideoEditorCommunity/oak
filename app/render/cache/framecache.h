
#include "render/texture.h"
#include <cstddef>
#include <ctime>
namespace olive::cache{
class FrameCacheKey{

};
class FrameCacheEntry{
private:
    TexturePtr texture_;
    time_t last_use;
public:
    FrameCacheEntry(){
        last_use = std::time(nullptr);
    };
    explicit FrameCacheEntry(TexturePtr texture){
        texture_ = texture;
		last_use = std::time(nullptr);
	}

};
}
