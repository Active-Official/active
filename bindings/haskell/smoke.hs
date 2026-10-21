{-# LANGUAGE ForeignFunctionInterface #-}
-- Haskell smoke binding (CHECKLIST Phase 2 stretch, PROPOSAL 2.1).
-- GHC `foreign import ccall` over the frozen C ABI: opaque handles stay
-- abstract Ptr (), every owned return is freed exactly once, borrowed
-- code strings are copied with peekCString and never freed.
-- Build: ghc -o smoke smoke.hs <libactive_ffi.a> ; run: ./smoke <lib>
module Main where

import Foreign
import Foreign.C
import System.Environment (getArgs)
import System.Exit (exitFailure)
import System.IO (hPutStrLn, stderr)
import Data.List (isInfixOf)
import Control.Exception (bracket)
import Control.Monad (unless, when)

foreign import ccall "active_abi_version" cAbi :: IO CUInt
foreign import ccall "active_module_parse" cParse :: CString -> CSize -> Ptr () -> IO (Ptr ())
foreign import ccall "active_module_free" cModFree :: Ptr () -> IO ()
foreign import ccall "active_check" cCheck :: Ptr () -> Ptr () -> IO (Ptr ())
foreign import ccall "active_eval" cEval :: Ptr () -> Ptr () -> IO (Ptr ())
foreign import ccall "active_diagnostics_len" cLen :: Ptr () -> IO CSize
foreign import ccall "active_diagnostic_code" cCode :: Ptr () -> CSize -> IO CString
foreign import ccall "active_diagnostics_free" cSetFree :: Ptr () -> IO ()
foreign import ccall "active_value_free" cValFree :: Ptr () -> IO ()
foreign import ccall "active_value_to_json" cToJson :: Ptr () -> IO CString
foreign import ccall "active_string_free" cStrFree :: CString -> IO ()

check :: Bool -> String -> IO ()
check True _ = return ()
check False msg = putStrLn ("SMOKE-FAIL: " ++ msg) >> exitFailure

withModule :: String -> (Ptr () -> IO ()) -> IO ()
withModule src act =
  withCStringLen src $ \(cs, n) ->
    bracket
      (do m <- cParse cs (fromIntegral n) nullPtr
          when (m == nullPtr) (check False "parse returned null")
          return m)
      cModFree
      act

ownedJson :: Ptr () -> IO String
ownedJson v = do
  p <- cToJson v
  when (p == nullPtr) (check False "to_json returned null")
  s <- peekCString p
  cStrFree p
  return s

main :: IO ()
main = do
  v <- cAbi
  check (v == 1) "ABI major mismatch"
  putStrLn "abi: 1 ok"
  withModule "port = 8080\n" $ \m -> do
    s <- cCheck m nullPtr
    when (s == nullPtr) (check False "check returned null")
    n <- cLen s
    cSetFree s
    check (n == 0) "expected clean"
  putStrLn "check clean ok"
  withModule "port = 8080\n" $ \m -> do
    val <- cEval m nullPtr
    when (val == nullPtr) (check False "eval returned null")
    json <- ownedJson val
    cValFree val
    check ("8080" `isInfixOf` json) ("value missing: " ++ json)
  putStrLn "eval port=8080 ok"
  withModule "timeout = 5s + 512MiB\n" $ \m -> do
    val <- cEval m nullPtr
    check (val == nullPtr) "units mixed evaluated clean"
    s <- cCheck m nullPtr
    when (s == nullPtr) (check False "check returned null")
    n <- cLen s
    code <- if n > 0 then peekCString =<< cCode s 0 else return ""
    cSetFree s
    check (n > 0) "expected diagnostics"
    check (code == "T001") ("expected T001, got " ++ code)
  putStrLn "units refused ok"
  withModule "token = secret(\"hunter2\")\n" $ \m -> do
    val <- cEval m nullPtr
    when (val == nullPtr) (check False "eval returned null")
    json <- ownedJson val
    cValFree val
    check ("***SECRET***" `isInfixOf` json && not ("hunter2" `isInfixOf` json))
          ("redaction broken: " ++ json)
  putStrLn "secret redacted ok"
  putStrLn "SMOKE-OK"
