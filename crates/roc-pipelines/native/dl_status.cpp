// Read-only DeckLink status queries (IDeckLinkStatus), used to learn the
// format actually arriving on an input while our capture pipeline holds it
// locked to another mode. Never enables video input; safe alongside
// decklinkvideosrc in the same process.

#include <cstdint>
#include <mutex>
#include <vector>

#include "DeckLinkAPI.h"

namespace {

std::mutex g_mu;
bool g_init = false;
// Index = DeckLink iterator order = GStreamer `device-number`.
std::vector<IDeckLinkStatus*> g_status;

void init_locked() {
    if (g_init) {
        return;
    }
    g_init = true;
    IDeckLinkIterator* it = CreateDeckLinkIteratorInstance();
    if (it == nullptr) {
        return;
    }
    IDeckLink* dl = nullptr;
    while (it->Next(&dl) == S_OK) {
        IDeckLinkStatus* st = nullptr;
        if (dl->QueryInterface(IID_IDeckLinkStatus, reinterpret_cast<void**>(&st)) != S_OK) {
            st = nullptr;
        }
        g_status.push_back(st);
        dl->Release();
    }
    it->Release();
}

}  // namespace

extern "C" {

struct roc_dl_status {
    int32_t available;
    int32_t locked;
    uint32_t detected_mode;  // BMDDisplayMode four-cc, 0 when not reported
    uint32_t current_mode;   // BMDDisplayMode four-cc, 0 when not reported
};

int32_t roc_dl_device_count() {
    std::lock_guard<std::mutex> guard(g_mu);
    init_locked();
    return static_cast<int32_t>(g_status.size());
}

// 0 on success, -1 when the index has no status interface.
int32_t roc_dl_query(uint32_t index, roc_dl_status* out) {
    if (out == nullptr) {
        return -1;
    }
    *out = roc_dl_status{};
    std::lock_guard<std::mutex> guard(g_mu);
    init_locked();
    if (index >= g_status.size() || g_status[index] == nullptr) {
        return -1;
    }
    IDeckLinkStatus* st = g_status[index];
    bool locked = false;
    if (st->GetFlag(bmdDeckLinkStatusVideoInputSignalLocked, &locked) == S_OK) {
        out->locked = locked ? 1 : 0;
    }
    int64_t v = 0;
    if (st->GetInt(bmdDeckLinkStatusDetectedVideoInputMode, &v) == S_OK) {
        out->detected_mode = static_cast<uint32_t>(v);
    }
    v = 0;
    if (st->GetInt(bmdDeckLinkStatusCurrentVideoInputMode, &v) == S_OK) {
        out->current_mode = static_cast<uint32_t>(v);
    }
    out->available = 1;
    return 0;
}

}  // extern "C"
