#include "framecache.h"
#include <mutex>
#include <qsysinfo.h>
#include <xcb/xproto.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif
#include <QSysInfo>

#ifdef _WIN32
#include <windows.h>
#elif defined(__linux__)
#include <stdio.h>
#include <string.h>
#else // macOS
#include <sys/sysctl.h>
#include <mach/mach.h>
#include <mach/vm_statistics.h>
#endif

uint64_t get_available_memory()
{
#ifdef _WIN32
	MEMORYSTATUSEX status;
	status.dwLength = sizeof(status);
	if (GlobalMemoryStatusEx(&status)) {
		return status.ullAvailPhys; 
	}
	return 0;

#elif defined(__linux__)
	FILE *fp = fopen("/proc/meminfo", "r");
	if (!fp) {
		return 0;
	}

	char line[256];
	uint64_t available_kb = 0;

	while (fgets(line, sizeof(line), fp)) {
		if (sscanf(line, "MemAvailable: %lu kB", &available_kb) == 1) {
			fclose(fp);
			return available_kb * 1024; 
		}
	}

	fclose(fp);
	return 0;

#else
	vm_size_t page_size;
	vm_statistics64_data_t vm_stats;
	mach_port_t mach_port = mach_host_self();
	mach_msg_type_number_t count = sizeof(vm_stats) / sizeof(natural_t);

    if (KERN_SUCCESS != host_page_size(mach_port, &page_size)) {
		return 0;
	}

	if (KERN_SUCCESS != host_statistics64(mach_port, HOST_VM_INFO64,
										  (host_info64_t)&vm_stats, &count)) {
		return 0;
	}

	return (uint64_t)vm_stats.free_count * page_size;
#endif
}

void my_sleep(unsigned int seconds)
{
#ifdef _WIN32
	Sleep(seconds * 1000);
#else
	sleep(seconds);
#endif
}

using namespace olive::cache;
FrameCache FrameCache::frame_cache_;

inline void my_sleep(){

}

void FrameCache::thread(){
    while(!stop){
        std::unique_lock<std::mutex> lock(size_lock);
        lock.lock();
		cache_size = get_available_memory() * 0.25;

        size_t total_cache_size = 0;

        for(auto entry: map_){
            total_cache_size += entry->size();
        }

        if(total_cache_size > cache_size){
            while(total_cache_size > cache_size){
                FrameCacheKey key = lru_list.last();
                lru_list.removeLast();
                size_t size=map_[key]->size();
                map_.remove(key);
                total_cache_size -= size;
            }
        }
        lock.unlock();
		my_sleep(5);
    }
}