
#include "olive/core/util/rational.h"
#include "render/texture.h"
#include "render/videoparams.h"
#include <cstddef>
#include <cstdint>
#include <ctime>
#include <memory>
#include <qhash.h>
#include <utility>
namespace olive::cache{
class FrameCacheKey{
private:
    uint64_t version_ = 0;
    rational time_ = 0;
    VideoParams params_;
public:
    FrameCacheKey() = default;
    FrameCacheKey(uint64_t version, rational time, VideoParams &params)
        : version_(version), time_(time), params_(params){}
    uint64_t version(){
        return version_;
    }
    rational time(){
        return time_;
    }
    VideoParams params(){
        return params_;
    }
};
class FrameCacheEntry{
private:
    TexturePtr texture_;
    time_t last_use;
public:
    FrameCacheEntry(){
        last_use = std::time(nullptr);
    };
    explicit FrameCacheEntry(TexturePtr &texture){
        texture_ = texture;
		last_use = std::time(nullptr);
	}

};

/*
 * LRU Frame Cache
 * TODO: This is just a mininum version. It should 
 * be replaced by a more complex one.
 */
class FrameCache{
    QHash<FrameCacheKey, FrameCacheEntry> map_;
    static FrameCache frame_cache_;
public:
    static FrameCache* getInstance(){
        return &frame_cache_;
    }
    FrameCache() = default;
    ~FrameCache() = default;

    FrameCacheEntry &get(FrameCacheKey& key){
        return map_[key];
    }
    void set(FrameCacheKey &key, FrameCacheEntry &entry){
        map_[key]=entry;
    }
};
}
