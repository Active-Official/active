# Perl smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
# FFI::Platypus over the frozen C ABI: opaque handles stay `opaque`,
# every owned return is freed exactly once (owned JSON is cast
# opaque->string, which copies, then freed), borrowed code strings are
# cast copies and never freed. Run: perl smoke.pl <libactive_ffi.so>
use strict;
use warnings;
use FFI::Platypus;

@ARGV == 1 or die "usage: smoke.pl <libactive_ffi.so>\n";
my $ffi = FFI::Platypus->new(api => 1);
$ffi->lib($ARGV[0]);

$ffi->attach(active_abi_version    => [] => 'uint32');
$ffi->attach(active_module_parse   => ['string', 'size_t', 'opaque'] => 'opaque');
$ffi->attach(active_module_free    => ['opaque'] => 'void');
$ffi->attach(active_check          => ['opaque', 'opaque'] => 'opaque');
$ffi->attach(active_eval           => ['opaque', 'opaque'] => 'opaque');
$ffi->attach(active_diagnostics_len => ['opaque'] => 'size_t');
$ffi->attach(active_diagnostic_code => ['opaque', 'size_t'] => 'opaque');
$ffi->attach(active_diagnostics_free => ['opaque'] => 'void');
$ffi->attach(active_value_free     => ['opaque'] => 'void');
$ffi->attach(active_value_to_json  => ['opaque'] => 'opaque');
$ffi->attach(active_string_free    => ['opaque'] => 'void');

sub check {
    my ($cond, $msg) = @_;
    if (!$cond) { print STDERR "SMOKE-FAIL: $msg\n"; exit 1; }
}

sub code_str {
    my ($ptr) = @_;
    return '' unless $ptr;
    return $ffi->cast('opaque', 'string', $ptr);
}

sub with_module {
    my ($src, $code) = @_;
    my $mod = active_module_parse($src, length($src), undef);
    check($mod, 'parse returned null');
    eval { $code->($mod); };
    my $err = $@;
    active_module_free($mod);
    die $err if $err;
}

sub owned_json {
    my ($val) = @_;
    my $ptr = active_value_to_json($val);
    check($ptr, 'to_json returned null');
    my $json = code_str($ptr);
    active_string_free($ptr);
    return $json;
}

check(active_abi_version() == 1, 'ABI major mismatch');
print "abi: 1 ok\n";

with_module("port = 8080\n", sub {
    my ($m) = @_;
    my $set = active_check($m, undef);
    check($set, 'check returned null');
    my $n = active_diagnostics_len($set);
    active_diagnostics_free($set);
    check($n == 0, 'expected clean');
});
print "check clean ok\n";

with_module("port = 8080\n", sub {
    my ($m) = @_;
    my $val = active_eval($m, undef);
    check($val, 'eval returned null');
    my $json = owned_json($val);
    active_value_free($val);
    check(index($json, '8080') >= 0, "value missing: $json");
});
print "eval port=8080 ok\n";

with_module("timeout = 5s + 512MiB\n", sub {
    my ($m) = @_;
    my $val = active_eval($m, undef);
    check(!$val, 'units mixed evaluated clean');
    my $set = active_check($m, undef);
    check($set, 'check returned null');
    my $n = active_diagnostics_len($set);
    my $code = $n > 0 ? code_str(active_diagnostic_code($set, 0)) : '';
    active_diagnostics_free($set);
    check($n > 0, 'expected diagnostics');
    check($code eq 'T001', "expected T001, got $code");
});
print "units refused ok\n";

with_module("token = secret(\"hunter2\")\n", sub {
    my ($m) = @_;
    my $val = active_eval($m, undef);
    check($val, 'eval returned null');
    my $json = owned_json($val);
    active_value_free($val);
    check(index($json, '***SECRET***') >= 0 && index($json, 'hunter2') < 0,
        "redaction broken: $json");
});
print "secret redacted ok\n";
print "SMOKE-OK\n";
