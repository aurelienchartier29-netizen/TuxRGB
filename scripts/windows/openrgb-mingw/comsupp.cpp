// SPDX-License-Identifier: GPL-2.0-or-later
//
// TuxRGB, for building OpenRGB with MinGW-w64 (build-openrgb-windows.sh).
//
// MinGW-w64 declares the string conversions of _com_util (comutil.h) but
// does not implement them; with MSVC they come from comsupp.lib. OpenRGB's
// WMI code uses them through _bstr_t. Same contract as Microsoft's: the BSTR
// is freed with SysFreeString, the string with delete[] (as _bstr_t does).

#include <windows.h>
#include <comutil.h>

namespace _com_util
{
BSTR WINAPI ConvertStringToBSTR(const char* src)
{
    if(src == nullptr)
    {
        return nullptr;
    }
    int len = MultiByteToWideChar(CP_ACP, 0, src, -1, nullptr, 0);
    if(len <= 0)
    {
        return nullptr;
    }
    BSTR out = SysAllocStringLen(nullptr, len - 1);
    if(out != nullptr)
    {
        MultiByteToWideChar(CP_ACP, 0, src, -1, out, len);
    }
    return out;
}

char* WINAPI ConvertBSTRToString(BSTR src)
{
    if(src == nullptr)
    {
        return nullptr;
    }
    int wlen = (int)SysStringLen(src);
    int len  = WideCharToMultiByte(CP_ACP, 0, src, wlen, nullptr, 0, nullptr, nullptr);
    char* out = new char[len + 1];
    WideCharToMultiByte(CP_ACP, 0, src, wlen, out, len, nullptr, nullptr);
    out[len] = '\0';
    return out;
}
}
