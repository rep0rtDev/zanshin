// libcurl transport for the Zanshin C++ SDK. Link with -lcurl; define ZANSHIN_CURL_INIT=0 if your app
// already calls curl_global_init.
#pragma once
#include <curl/curl.h>

#include <mutex>

#include "zanshin.hpp"

namespace zanshin {

class CurlTransport : public HttpTransport {
public:
    CurlTransport() {
        static std::once_flag once;
#ifndef ZANSHIN_CURL_INIT
#define ZANSHIN_CURL_INIT 1
#endif
        if (ZANSHIN_CURL_INIT) std::call_once(once, [] { curl_global_init(CURL_GLOBAL_DEFAULT); });
    }

    HttpResponse send(const HttpRequest& req) override {
        HttpResponse out;
        CURL* c = curl_easy_init();
        if (!c) { out.error = "curl_easy_init failed"; return out; }
        struct curl_slist* hdrs = nullptr;
        for (const auto& h : req.headers) hdrs = curl_slist_append(hdrs, (h.first + ": " + h.second).c_str());
        curl_easy_setopt(c, CURLOPT_URL, req.url.c_str());
        curl_easy_setopt(c, CURLOPT_HTTPHEADER, hdrs);
        curl_easy_setopt(c, CURLOPT_CUSTOMREQUEST, req.method.c_str());
        curl_easy_setopt(c, CURLOPT_TIMEOUT_MS, static_cast<long>(req.timeout_ms));
        curl_easy_setopt(c, CURLOPT_FOLLOWLOCATION, 0L);
        curl_easy_setopt(c, CURLOPT_ACCEPT_ENCODING, "");
        if (!req.body.empty() || req.method == "POST" || req.method == "PUT") {
            curl_easy_setopt(c, CURLOPT_POSTFIELDS, req.body.c_str());
            curl_easy_setopt(c, CURLOPT_POSTFIELDSIZE, static_cast<long>(req.body.size()));
        }
        curl_easy_setopt(c, CURLOPT_WRITEFUNCTION, +[](char* p, size_t s, size_t n, void* ud) -> size_t {
            static_cast<std::string*>(ud)->append(p, s * n);
            return s * n;
        });
        curl_easy_setopt(c, CURLOPT_WRITEDATA, &out.body);
        CURLcode rc = curl_easy_perform(c);
        if (rc != CURLE_OK) {
            out.status = 0;
            out.error = curl_easy_strerror(rc);
        } else {
            long status = 0;
            curl_easy_getinfo(c, CURLINFO_RESPONSE_CODE, &status);
            out.status = static_cast<int>(status);
        }
        curl_slist_free_all(hdrs);
        curl_easy_cleanup(c);
        return out;
    }
};

}  // namespace zanshin
