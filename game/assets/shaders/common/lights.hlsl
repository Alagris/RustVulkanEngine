#ifndef GAME_LIGHTS_HLSL
#define GAME_LIGHTS_HLSL

float3 GetDirectionalLightDirection()
{
    return normalize(-directionalLightDirection.xyz);
}

float3 GetDirectionalLightColor()
{
    return directionalLightColorIntensity.rgb;
}

float GetDirectionalLightIntensity()
{
    return directionalLightColorIntensity.w;
}

#endif
