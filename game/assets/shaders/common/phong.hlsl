#ifndef GAME_PHONG_HLSL
#define GAME_PHONG_HLSL

#include "lights.hlsl"

float3 ComputePhongLighting(
    float3 albedo,
    float3 normal,
    float3 viewDirection,
    float shininess,
    float visibility)
{
    float3 lightDirection = GetDirectionalLightDirection();
    float3 normalizedNormal = normalize(normal);
    float3 normalizedViewDirection = normalize(viewDirection);
    float diffuseFactor = max(dot(normalizedNormal, lightDirection), 0.0);
    float3 reflectedLight = reflect(-lightDirection, normalizedNormal);
    float specularFactor = pow(
        max(dot(normalizedViewDirection, reflectedLight), 0.0),
        max(shininess, 1.0));
    float3 directLight = GetDirectionalLightColor() * GetDirectionalLightIntensity();
    float3 ambient = albedo * 0.08;
    float3 diffuse = albedo * directLight * diffuseFactor;
    float3 specular = directLight * specularFactor;
    return ambient + (diffuse + specular) * saturate(visibility);
}

#endif
