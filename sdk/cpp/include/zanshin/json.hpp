// Minimal JSON value + parser/serializer for the Zanshin C++ SDK (C++17, header-only, no deps).
#pragma once
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <map>
#include <memory>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace zanshin {

class Json {
public:
    enum class Type { Null, Bool, Number, String, Array, Object };
    using Array = std::vector<Json>;
    using Object = std::map<std::string, Json>;

    Json() = default;
    Json(std::nullptr_t) {}
    Json(bool b) : type_(Type::Bool), bool_(b) {}
    Json(int n) : type_(Type::Number), num_(n) {}
    Json(long n) : type_(Type::Number), num_(static_cast<double>(n)) {}
    Json(long long n) : type_(Type::Number), num_(static_cast<double>(n)) {}
    Json(double n) : type_(Type::Number), num_(n) {}
    Json(const char* s) : type_(Type::String), str_(s) {}
    Json(std::string s) : type_(Type::String), str_(std::move(s)) {}
    Json(Array a) : type_(Type::Array), arr_(std::make_shared<Array>(std::move(a))) {}
    Json(Object o) : type_(Type::Object), obj_(std::make_shared<Object>(std::move(o))) {}
    Json(std::initializer_list<std::pair<const std::string, Json>> o)
        : type_(Type::Object), obj_(std::make_shared<Object>(o)) {}

    static Json array() { return Json(Array{}); }
    static Json object() { return Json(Object{}); }

    Type type() const { return type_; }
    bool is_null() const { return type_ == Type::Null; }
    bool is_bool() const { return type_ == Type::Bool; }
    bool is_number() const { return type_ == Type::Number; }
    bool is_string() const { return type_ == Type::String; }
    bool is_array() const { return type_ == Type::Array; }
    bool is_object() const { return type_ == Type::Object; }

    bool as_bool(bool fallback = false) const {
        if (type_ == Type::Bool) return bool_;
        if (type_ == Type::String) return str_ == "true";
        if (type_ == Type::Number) return num_ != 0;
        return fallback;
    }
    double as_number(double fallback = 0) const {
        if (type_ == Type::Number) return num_;
        if (type_ == Type::String) {
            char* end = nullptr;
            double v = std::strtod(str_.c_str(), &end);
            if (end && *end == '\0' && !str_.empty()) return v;
        }
        return fallback;
    }
    long long as_int(long long fallback = 0) const {
        return is_number() || is_string() ? static_cast<long long>(as_number(static_cast<double>(fallback))) : fallback;
    }
    const std::string& as_string() const {
        static const std::string empty;
        return type_ == Type::String ? str_ : empty;
    }
    std::string as_string(const std::string& fallback) const { return type_ == Type::String ? str_ : fallback; }
    const Array& as_array() const {
        static const Array empty;
        return type_ == Type::Array ? *arr_ : empty;
    }
    const Object& as_object() const {
        static const Object empty;
        return type_ == Type::Object ? *obj_ : empty;
    }
    Array& as_array() {
        if (type_ != Type::Array) { type_ = Type::Array; arr_ = std::make_shared<Array>(); }
        return *arr_;
    }
    Object& as_object() {
        if (type_ != Type::Object) { type_ = Type::Object; obj_ = std::make_shared<Object>(); }
        return *obj_;
    }

    /// Member lookup; a missing key yields a shared null.
    const Json& operator[](const std::string& key) const {
        static const Json null;
        if (type_ != Type::Object) return null;
        auto it = obj_->find(key);
        return it == obj_->end() ? null : it->second;
    }
    Json& operator[](const std::string& key) { return as_object()[key]; }
    const Json& operator[](const char* key) const { return (*this)[std::string(key)]; }
    Json& operator[](const char* key) { return as_object()[key]; }
    const Json& operator[](size_t i) const {
        static const Json null;
        if (type_ != Type::Array || i >= arr_->size()) return null;
        return (*arr_)[i];
    }
    const Json& operator[](int i) const { return (*this)[static_cast<size_t>(i < 0 ? 0 : i)]; }
    /// Mutable index: grows the array as needed.
    Json& operator[](size_t i) {
        auto& a = as_array();
        if (i >= a.size()) a.resize(i + 1);
        return a[i];
    }
    Json& operator[](int i) { return (*this)[static_cast<size_t>(i < 0 ? 0 : i)]; }
    bool contains(const std::string& key) const { return type_ == Type::Object && obj_->count(key) > 0; }
    size_t size() const { return type_ == Type::Array ? arr_->size() : type_ == Type::Object ? obj_->size() : 0; }

    std::string dump() const {
        std::string out;
        write(out);
        return out;
    }

    static Json parse(const std::string& text) {
        Parser p{text, 0};
        p.ws();
        Json v = p.value();
        p.ws();
        if (p.i != text.size()) throw std::runtime_error("json: trailing characters");
        return v;
    }
    /// Non-throwing parse; returns null and sets ok=false on error.
    static Json try_parse(const std::string& text, bool& ok) {
        try { ok = true; return parse(text); } catch (const std::exception&) { ok = false; return Json(); }
    }

private:
    Type type_ = Type::Null;
    bool bool_ = false;
    double num_ = 0;
    std::string str_;
    std::shared_ptr<Array> arr_;
    std::shared_ptr<Object> obj_;

    static void write_string(std::string& out, const std::string& s) {
        out += '"';
        for (unsigned char c : s) {
            switch (c) {
                case '"': out += "\\\""; break;
                case '\\': out += "\\\\"; break;
                case '\n': out += "\\n"; break;
                case '\r': out += "\\r"; break;
                case '\t': out += "\\t"; break;
                case '\b': out += "\\b"; break;
                case '\f': out += "\\f"; break;
                default:
                    if (c < 0x20) { char buf[8]; std::snprintf(buf, sizeof buf, "\\u%04x", c); out += buf; }
                    else out += static_cast<char>(c);
            }
        }
        out += '"';
    }
    void write(std::string& out) const {
        switch (type_) {
            case Type::Null: out += "null"; break;
            case Type::Bool: out += bool_ ? "true" : "false"; break;
            case Type::Number: {
                if (std::isfinite(num_) && num_ == std::floor(num_) && std::fabs(num_) < 1e15) {
                    out += std::to_string(static_cast<long long>(num_));
                } else {
                    char buf[32]; std::snprintf(buf, sizeof buf, "%.17g", num_); out += buf;
                }
                break;
            }
            case Type::String: write_string(out, str_); break;
            case Type::Array: {
                out += '[';
                bool first = true;
                for (const auto& v : *arr_) { if (!first) out += ','; first = false; v.write(out); }
                out += ']';
                break;
            }
            case Type::Object: {
                out += '{';
                bool first = true;
                for (const auto& kv : *obj_) { if (!first) out += ','; first = false; write_string(out, kv.first); out += ':'; kv.second.write(out); }
                out += '}';
                break;
            }
        }
    }

    struct Parser {
        const std::string& s;
        size_t i;
        void ws() { while (i < s.size() && (s[i] == ' ' || s[i] == '\n' || s[i] == '\r' || s[i] == '\t')) ++i; }
        [[noreturn]] void fail(const char* what) { throw std::runtime_error(std::string("json: ") + what + " at " + std::to_string(i)); }
        bool lit(const char* w) {
            size_t n = std::char_traits<char>::length(w);
            if (s.compare(i, n, w) == 0) { i += n; return true; }
            return false;
        }
        Json value() {
            if (i >= s.size()) fail("unexpected end");
            char c = s[i];
            if (c == '{') return object();
            if (c == '[') return array();
            if (c == '"') return Json(string());
            if (c == 't') { if (lit("true")) return Json(true); fail("bad literal"); }
            if (c == 'f') { if (lit("false")) return Json(false); fail("bad literal"); }
            if (c == 'n') { if (lit("null")) return Json(); fail("bad literal"); }
            return number();
        }
        Json number() {
            size_t start = i;
            if (s[i] == '-') ++i;
            while (i < s.size() && (std::isdigit(static_cast<unsigned char>(s[i])) || s[i] == '.' || s[i] == 'e' || s[i] == 'E' || s[i] == '+' || s[i] == '-')) ++i;
            if (start == i) fail("unexpected character");
            return Json(std::strtod(s.substr(start, i - start).c_str(), nullptr));
        }
        static void utf8(std::string& out, unsigned cp) {
            if (cp < 0x80) out += static_cast<char>(cp);
            else if (cp < 0x800) { out += static_cast<char>(0xC0 | (cp >> 6)); out += static_cast<char>(0x80 | (cp & 0x3F)); }
            else if (cp < 0x10000) { out += static_cast<char>(0xE0 | (cp >> 12)); out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F)); out += static_cast<char>(0x80 | (cp & 0x3F)); }
            else { out += static_cast<char>(0xF0 | (cp >> 18)); out += static_cast<char>(0x80 | ((cp >> 12) & 0x3F)); out += static_cast<char>(0x80 | ((cp >> 6) & 0x3F)); out += static_cast<char>(0x80 | (cp & 0x3F)); }
        }
        unsigned hex4() {
            if (i + 4 > s.size()) fail("bad \\u escape");
            unsigned v = 0;
            for (int k = 0; k < 4; ++k) {
                char c = s[i++]; v <<= 4;
                if (c >= '0' && c <= '9') v |= c - '0';
                else if (c >= 'a' && c <= 'f') v |= c - 'a' + 10;
                else if (c >= 'A' && c <= 'F') v |= c - 'A' + 10;
                else fail("bad hex digit");
            }
            return v;
        }
        std::string string() {
            ++i;  // opening quote
            std::string out;
            while (true) {
                if (i >= s.size()) fail("unterminated string");
                char c = s[i++];
                if (c == '"') return out;
                if (c != '\\') { out += c; continue; }
                if (i >= s.size()) fail("bad escape");
                char e = s[i++];
                switch (e) {
                    case '"': out += '"'; break;
                    case '\\': out += '\\'; break;
                    case '/': out += '/'; break;
                    case 'b': out += '\b'; break;
                    case 'f': out += '\f'; break;
                    case 'n': out += '\n'; break;
                    case 'r': out += '\r'; break;
                    case 't': out += '\t'; break;
                    case 'u': {
                        unsigned cp = hex4();
                        if (cp >= 0xD800 && cp <= 0xDBFF && i + 6 <= s.size() && s[i] == '\\' && s[i + 1] == 'u') {
                            i += 2;
                            unsigned lo = hex4();
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        }
                        utf8(out, cp);
                        break;
                    }
                    default: fail("bad escape");
                }
            }
        }
        Json array() {
            ++i;
            Array out;
            ws();
            if (i < s.size() && s[i] == ']') { ++i; return Json(std::move(out)); }
            while (true) {
                ws();
                out.push_back(value());
                ws();
                if (i >= s.size()) fail("unterminated array");
                if (s[i] == ',') { ++i; continue; }
                if (s[i] == ']') { ++i; return Json(std::move(out)); }
                fail("expected , or ]");
            }
        }
        Json object() {
            ++i;
            Object out;
            ws();
            if (i < s.size() && s[i] == '}') { ++i; return Json(std::move(out)); }
            while (true) {
                ws();
                if (i >= s.size() || s[i] != '"') fail("expected key");
                std::string k = string();
                ws();
                if (i >= s.size() || s[i] != ':') fail("expected :");
                ++i;
                ws();
                out[k] = value();
                ws();
                if (i >= s.size()) fail("unterminated object");
                if (s[i] == ',') { ++i; continue; }
                if (s[i] == '}') { ++i; return Json(std::move(out)); }
                fail("expected , or }");
            }
        }
    };
};

}  // namespace zanshin
