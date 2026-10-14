CXX ?= g++
CXXFLAGS ?= -std=c++17 -O1 -g -Wall -Wextra -pedantic -Iinclude

test: build/zanshin_tests
	./build/zanshin_tests

build/zanshin_tests: tests/test.cpp include/zanshin/zanshin.hpp include/zanshin/json.hpp
	@mkdir -p build
	$(CXX) $(CXXFLAGS) tests/test.cpp -o $@

# Compile-check the libcurl adapter (needs libcurl headers).
curl-check:
	echo '#include <zanshin/curl_transport.hpp>\nint main(){}' | $(CXX) $(CXXFLAGS) -x c++ - -o /dev/null -lcurl

clean:
	rm -rf build
.PHONY: test curl-check clean
