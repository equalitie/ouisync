# Limitations

Currently only works with Boost stackful coroutines

# Dependencies

* [Boost](https://www.boost.org/) >= 1.88.0
* [msgpack](https://github.com/msgpack/msgpack-c) (C++)

# Build & run

```bash
# Optional
CMAKE_OPTIONS=(
    # Use these lines if your OS doesn't provide the required
    # Boost package version
    -DBOOST_INCLUDEDIR=<WHERE YOU BUILT BOOST>/boost/install/include
    -DBOOST_LIBRARYDIR=<WHERE YOU BUILT BOOST>/boost/install/lib
    -DBoost_NO_SYSTEM_PATHS=ON
    # Fail compilation in presence of warnings (Optional)
    -DWARNING_IS_ERROR=ON
    # Show how long it took to build cpp files and link them (Optional)
    -DMEASURE_BUILD_TIME=ON
)

cd bindings/cpp
mkdir -p build
cd build

cmake .. ${CMAKE_OPTIONS[@]}
cmake --build . -j`nproc`

# Run tests
ctest --output-on-failure

# Run example
./example
```

# TODO

* Improve Doxygen documentation
* std::ostream for types in data.hpp
* Remove copy constructors where appropriate
* nothrow constructors where appropriate
