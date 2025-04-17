# C example programs

This directory contains the source code for example programs in C that show how to obtain error bounded timestamps from the ClockBound daemon. The example programs make use of the libclockbound C library that is produced by `clock-bound-ffi`.

## Prerequisites

- `gcc` is required for compiling C source code files. Use following command to install it if you don't have it:

  ```sh
  sudo yum install gcc
  ```

- The ClockBound daemon must be running for the example to work.
  See the [ClockBound daemon documentation](../../clock-bound-d/README.md) for
  details on how to get the ClockBound daemon running.

- `libclockbound` library is required for the example to work. See the [ClockBound FFI documentation](../../clock-bound-ffi/README.md#building) for details on how to build the `libclockbound` library.

- Specify the directories to be searched for shared libraries in the `LD_LIBRARY_PATH`. Add following to your shell configuration file. See `.zshrc` example:

  ```sh
  vim ~/.zshrc
  
  # Add following line to the shell configuration file
  export LD_LIBRARY_PATH=/usr/lib
  
  # Use updated shell configuration
  source ~/.zshrc
  ```

## Running

- Run the following command to compile example C source code files.

  ```sh
  # From top-level directory cd into src directory that contains examples in C.
  cd examples/client/c/src
  
  # Compile the C source code files.
  gcc clockbound_now.c -o clockbound_now -I/usr/include -L/usr/lib -lclockbound
  gcc clockbound_loop_forever.c -o clockbound_loop_forever -I/usr/include -L/usr/lib -lclockbound
  ```

- Run the following command to run the C example programs.

  ```sh
  # Run the `clockbound_now` program.
  ./clockbound_now

  # The output should look something like the following:
  When clockbound_now was called true time was somewhere within 1709854392.907495824 and 1709854392.908578628 seconds since Jan 1 1970. The clock status is SYNCHRONIZED.
  It took 9.428327416 seconds to call clock bound 100000000 times (10606335 tps).
  ```

  ```sh
  # Run the `clockbound_loop_forever` program.
  ./clockbound_loop_forever

  # The output should look something like the following:
  When clockbound_now was called true time was somewhere within 1741187470.034504209 and 1741187470.035652589 seconds since Jan 1 1970. The clock status is SYNCHRONIZED (1).
  When clockbound_now was called true time was somewhere within 1741187471.034596805 and 1741187471.035746587 seconds since Jan 1 1970. The clock status is SYNCHRONIZED (1).
  When clockbound_now was called true time was somewhere within 1741187472.034682964 and 1741187472.035834148 seconds since Jan 1 1970. The clock status is SYNCHRONIZED (1).

  # To quit the example program, press CTRL-C.
  ```

- Clean up

  ```sh
  rm ./clockbound_now
  rm ./clockbound_loop_forever
  ```
