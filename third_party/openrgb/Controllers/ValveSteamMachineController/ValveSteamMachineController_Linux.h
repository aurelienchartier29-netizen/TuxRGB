/*---------------------------------------------------------*\
| ValveSteamMachineController_Linux.h                       |
|                                                           |
|   Driver for Valve Steam Machine LEDs                     |
|                                                           |
|   Adam Honse (calcprogrammer1@gmail.com)      23 Jul 2026 |
|                                                           |
|   This file is part of the OpenRGB project                |
|   SPDX-License-Identifier: GPL-2.0-or-later               |
\*---------------------------------------------------------*/

#pragma once

#include <fstream>
#include <string>
#include <vector>
#include "RGBControllerInterface.h"

class ValveSteamMachineController
{
public:
    ValveSteamMachineController(std::string dev_name);
    ~ValveSteamMachineController();

    std::string                 GetName();

    void                        AddLED(std::string led_path);
    size_t                      GetLEDCount();
    std::string                 GetLocation();

    std::vector<std::string>    GetAvailableEffects();
    unsigned int                GetBrightness();
    unsigned int                GetDelay();
    std::string                 GetEffect();
    bool                        GetEnabled();
    unsigned int                GetBreathOffset();
    unsigned int                GetBreathLevel();
    unsigned int                GetPatrolNum();
    unsigned int                GetColorShift();
    RGBColor                    GetStartupColor();
    unsigned int                GetBrightnessStartup();

    void                        SetLEDColor(unsigned int led_idx, RGBColor color);
    void                        SetEffect(std::string effect);
    void                        SetBrightness(unsigned int brightness);
    void                        SetDelay(unsigned int delay);
    void                        SetEnabled(bool enabled);
    void                        SetBreathOffset(unsigned int breath_offset);
    void                        SetBreathLevel(unsigned int breath_level);
    void                        SetPatrolNum(unsigned int patrol_num);
    void                        SetColorShift(unsigned int color_shift);
    void                        SetStartupColor(RGBColor color);
    void                        SetBrightnessStartup(unsigned int brightness_startup);

private:
    std::string                 name;

    std::vector<std::string>    led_paths;
    std::vector<std::ofstream>  led_multi_intensity;
    std::vector<std::ofstream>  led_effect;
    std::ofstream               led_brightness_scale;
    std::ofstream               led_delay;
    std::ofstream               led_enabled;
    std::ofstream               led_breath_offset;
    std::ofstream               led_breath_level;
    std::ofstream               led_patrol_num;
    std::ofstream               led_color_shift;
    std::ofstream               led_multi_intensity_startup;
    std::ofstream               led_brightness_startup;
    std::vector<std::string>    available_effects;

    void                        ReadAvailableEffects(std::string first_led_path);
};
