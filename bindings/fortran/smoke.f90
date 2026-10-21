! Fortran smoke binding (CHECKLIST 4.001 stretch, PROPOSAL section 2.1).
! ISO_C_BINDING interfaces over the frozen C ABI: opaque handles as
! TYPE(C_PTR), ABI major gate at load, Allocator Model A (every owned
! pointer freed exactly once on every path, including assertion exits).
! Compile and run (Linux, gfortran):
!   gfortran bindings/fortran/smoke.f90 -I active-ffi \
!     target/x86_64-unknown-linux-gnu/release/libactive_ffi.a \
!     -o /tmp/active-fortran-smoke && /tmp/active-fortran-smoke
module active_ffi
  use, intrinsic :: iso_c_binding, only: c_int32_t, c_int64_t, c_char, &
    c_ptr, c_null_ptr, c_null_char, c_associated, c_f_pointer, c_size_t
  implicit none

  type, bind(C) :: ActiveBudget
    integer(c_int32_t) :: struct_size
    integer(c_int64_t) :: max_nodes, max_depth, max_array_len, max_keys
    integer(c_int64_t) :: max_wall_ms, max_value_size, max_total_value_size
  end type ActiveBudget

  interface
    function active_abi_version() bind(C)
      import c_int32_t
      integer(c_int32_t) :: active_abi_version
    end function
    function active_module_parse(src, n, options) bind(C)
      import c_char, c_int64_t, c_ptr
      character(kind=c_char), dimension(*), intent(in) :: src
      integer(c_int64_t), value :: n
      type(c_ptr), value :: options
      type(c_ptr) :: active_module_parse
    end function
    subroutine active_module_free(h) bind(C)
      import c_ptr
      type(c_ptr), value :: h
    end subroutine
    function active_check(m, budget) bind(C)
      import c_ptr
      type(c_ptr), value :: m, budget
      type(c_ptr) :: active_check
    end function
    function active_diagnostics_len(s) bind(C)
      import c_ptr, c_int64_t
      type(c_ptr), value :: s
      integer(c_int64_t) :: active_diagnostics_len
    end function
    function active_diagnostic_code(s, i) bind(C)
      import c_ptr, c_int64_t
      type(c_ptr), value :: s
      integer(c_int64_t), value :: i
      type(c_ptr) :: active_diagnostic_code
    end function
    subroutine active_diagnostics_free(s) bind(C)
      import c_ptr
      type(c_ptr), value :: s
    end subroutine
    function active_eval(m, budget) bind(C)
      import c_ptr
      type(c_ptr), value :: m, budget
      type(c_ptr) :: active_eval
    end function
    function active_value_to_json(v) bind(C)
      import c_ptr
      type(c_ptr), value :: v
      type(c_ptr) :: active_value_to_json
    end function
    subroutine active_value_free(v) bind(C)
      import c_ptr
      type(c_ptr), value :: v
    end subroutine
    subroutine active_string_free(s) bind(C)
      import c_ptr
      type(c_ptr), value :: s
    end subroutine
  end interface
end module active_ffi

program smoke
  use, intrinsic :: iso_c_binding, only: c_associated, c_null_ptr, c_char, c_null_char
  use active_ffi
  implicit none
  character(len=:), allocatable :: json

  call require_version()
  call check_clean()
  call check_eval()
  call check_refused()
  call check_redacted()
  print *, "SMOKE-OK"

contains

  subroutine fail(msg)
    character(len=*), intent(in) :: msg
    print *, "SMOKE-FAIL: " // msg
    error stop 1
  end subroutine fail

  subroutine require_version()
    if (active_abi_version() /= 1) call fail("ABI major mismatch")
    print *, "abi: 1 ok"
  end subroutine require_version

  function cstr(p) result(s)
    type(c_ptr), intent(in) :: p
    character(len=:), allocatable :: s
    character(kind=c_char), pointer :: chars(:)
    integer :: n
    if (.not. c_associated(p)) then
      s = ""
      return
    end if
    call c_f_pointer(p, chars, [huge(0)])
    n = 1
    do while (n <= size(chars) .and. chars(n) /= c_null_char)
      n = n + 1
    end do
    allocate (character(len=n-1) :: s)
    block
      integer :: i
      do i = 1, n - 1
        s(i:i) = chars(i)
      end do
    end block
  end function cstr

  subroutine check_clean()
    type(c_ptr) :: m, s
    m = parse("port = 8080" // new_line("a"))
    s = active_check(m, c_null_ptr)
    if (.not. c_associated(s)) call fail("check returned null")
    if (active_diagnostics_len(s) /= 0) call fail("expected clean")
    call active_diagnostics_free(s)
    call active_module_free(m)
    print *, "check clean ok"
  end subroutine check_clean

  subroutine check_eval()
    type(c_ptr) :: m, v, j
    character(len=:), allocatable :: json
    m = parse("port = 8080" // new_line("a"))
    v = active_eval(m, c_null_ptr)
    if (.not. c_associated(v)) call fail("eval returned null")
    j = active_value_to_json(v)
    if (.not. c_associated(j)) call fail("to_json returned null")
    json = cstr(j)
    call active_string_free(j)
    call active_value_free(v)
    call active_module_free(m)
    if (index(json, "8080") == 0) call fail("value missing: " // json)
    print *, "eval port=8080 ok"
  end subroutine check_eval

  subroutine check_refused()
    type(c_ptr) :: m, v, s, code
    character(len=:), allocatable :: c
    m = parse("timeout = 5s + 512MiB" // new_line("a"))
    v = active_eval(m, c_null_ptr)
    if (c_associated(v)) call fail("units mixed evaluated clean")
    s = active_check(m, c_null_ptr)
    if (.not. c_associated(s)) call fail("check returned null")
    if (active_diagnostics_len(s) < 1) call fail("expected diagnostics")
    code = active_diagnostic_code(s, 0_c_int64_t)
    c = cstr(code)
    if (c /= "T001") call fail("expected T001, got " // c)
    call active_diagnostics_free(s)
    call active_module_free(m)
    print *, "units refused ok"
  end subroutine check_refused

  subroutine check_redacted()
    type(c_ptr) :: m, v, j
    character(len=:), allocatable :: json
    m = parse('token = secret("hunter2")' // new_line("a"))
    v = active_eval(m, c_null_ptr)
    if (.not. c_associated(v)) call fail("eval returned null")
    j = active_value_to_json(v)
    if (.not. c_associated(j)) call fail("to_json returned null")
    json = cstr(j)
    call active_string_free(j)
    call active_value_free(v)
    call active_module_free(m)
    if (index(json, "***SECRET***") == 0) call fail("redaction missing")
    if (index(json, "hunter2") /= 0) call fail("raw secret leaked")
    print *, "secret redacted ok"
  end subroutine check_redacted

  function parse(src) result(m)
    character(len=*), intent(in) :: src
    type(c_ptr) :: m
    integer :: n, i
    character(kind=c_char), allocatable :: buf(:)
    n = len(src)
    allocate (buf(n + 1))
    do i = 1, n
      buf(i) = src(i:i)
    end do
    buf(n + 1) = c_null_char
    m = active_module_parse(buf, int(n, c_int64_t), c_null_ptr)
    if (.not. c_associated(m)) call fail("parse returned null")
  end function parse

end program smoke
