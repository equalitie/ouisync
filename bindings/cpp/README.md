# Limitations

Currently only works with Boost stackful coroutines

# Dependencies

* Boost >= 1.88.0
* messagepack

# Build & run

```bash
# Optional
CMAKE_OPTIONS=(
    # Use these lines if your OS doesn't provide the required
    # Boost package version
    -DBOOST_INCLUDEDIR=<WHERE YOU BUILT BOOST>/boost/install/include
    -DBOOST_LIBRARYDIR=<WHERE YOU BUILT BOOST>/boost/install/lib
    -DBoost_NO_SYSTEM_PATHS=TRUE
)

BUILD_DIR=<WHERE YOU WANT YOUR BUILD TO HAPPEN>
SRC_DIR=<WHERE THE example/CMakeLists.txt IS>

mkdir -p $BUILD_DIR
cd $BUILD_DIR

cmake $SRC_DIR ${CMAKE_OPTIONS[@]}
cmake --build . -j`nproc`

./example
```

# TODO

* Improve Doxygen documentation
* std::ostream for types in data.hpp
* Remove copy constructors where appropriate
* nothrow constructors where appropriate
