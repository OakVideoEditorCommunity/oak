
#include "gtest/gtest.h"
#include "olive/core/util/rational.h"
#include "render/texture.h"
#include "render/videoparams.h"
#include <cstddef>
#include <cstdint>
#include <ctime>
#include <functional>
#include <memory>
#include <mutex>
#include <QHash>
#include <QThread>
#include <qobject.h>
#include <qtmetamacros.h>
#include <thread>
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
    bool operator==(FrameCacheKey &other){
        return version_==other.version_ && time_ == other.time_
            && params_ == other.params_;
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
    TexturePtr texture(){
        last_use = std::time(nullptr);
        return texture_;
    }
    time_t last_use_time(){
        return last_use;
    }

    size_t size(){
        int byte_per_channel = texture_->format().byte_count();
        int width = texture_->width();
        int height = texture_->height();
        int channel_count = texture_->channel_count();
		int overhead_factor = 1.3; // std::shared_ptr, metadata
        return byte_per_channel * width * height * channel_count * overhead_factor;
	}

};
using FrameCacheEntryPtr = std::shared_ptr<FrameCacheEntry>;
/*
 * LRU Frame Cache
 * TODO: This is just a mininum version. It should 
 * be replaced by a more complex one.
 * We use 25% of availiable memory as LRU Cache.
 */
	class FrameCache {
private:
    QHash<FrameCacheKey, FrameCacheEntryPtr> map_;
    static FrameCache frame_cache_;
    std::mutex map_lock;
    std::mutex size_lock;
    size_t cache_size;
    bool stop = false;
    std::thread *gc_thread;
    QList<FrameCacheKey> lru_list;
public:
    static FrameCache* getInstance(){
        return &frame_cache_;
    }
    FrameCache(){
        auto f = std::bind(&FrameCache::thread, this);
        gc_thread=new std::thread(f);
    };
    ~FrameCache(){
        finalize();
    };

    FrameCacheEntryPtr get(FrameCacheKey& key){

        if(map_.contains(key)){
			if (lru_list.last() != key) {
				lru_list.removeOne(key);
				lru_list.append(key);
			}
			return map_[key];
		}
        else{
            return nullptr;
        }
        
    }
    void set(FrameCacheKey &key, FrameCacheEntryPtr entry){
        std::lock_guard<std::mutex> lock(this->map_lock);
        map_[key]=entry;
        if(!lru_list.contains(key)){
            lru_list.append(key);
        }
        else {
            lru_list.removeOne(key);
			lru_list.append(key);
		}
    }
    void remove(FrameCacheKey &key){
		std::lock_guard<std::mutex> lock(this->map_lock);
        lru_list.removeOne(key);
		map_.remove(key);
    }

    void finalize(){
        stop = true;
        gc_thread->join();
    }
protected:
    void thread();
};
}
